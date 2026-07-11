use std::sync::Arc;

use aether_example_energy_gateway::{EnergyGateway, PersistenceForecastProcessor};
use aether_sdk::application::{
    Actor, DataProcessingApplication, DataProcessingBinding, DataProcessingRoute,
    PointFeatureBinding, RequestContext, SafetyPolicy,
};
use aether_sdk::domain::{
    BindingIdentity, FeatureDefinition, FeatureRole, FeatureValue, ForecastOptions,
    ProcessTaskRequest, ProcessingOptions, ProcessingOutput, ProcessingStatus, SampleQuality,
    Segment, SegmentKind, Series, SourceKind, SourceProvenance, TimestampMs,
};
use aether_sdk::ports::SourcedSegment;
use aether_store_local::{
    ManualClock, MemoryAuditSink, MemoryCovariateSource, MemoryHistoryQuery, MemoryLiveState,
};

fn series(definition: FeatureDefinition, values: Vec<f64>) -> Series {
    let sample_count = values.len();
    Series::new(
        definition,
        values
            .into_iter()
            .map(|value| FeatureValue::number(value).expect("value is finite"))
            .collect(),
        vec![SampleQuality::Good; sample_count],
    )
    .expect("series is valid")
}

fn history_values(feature: &str, sample_count: usize) -> Vec<f64> {
    (0..sample_count)
        .map(|index| match feature {
            "load" => 800.0 + index as f64,
            "temp_avg" => 20.0 + (index % 48) as f64 / 10.0,
            "humidity" => 55.0 + (index % 20) as f64,
            "rain" => usize::from(index % 97 == 0) as f64 / 10.0,
            "quarter_hour" => ((index + 1) % 96) as f64,
            other => panic!("unexpected history feature {other}"),
        })
        .collect()
}

fn future_values(feature: &str, sample_count: usize) -> Vec<f64> {
    (0..sample_count)
        .map(|index| match feature {
            "temp_avg" => 26.0 + index as f64 / 10.0,
            "humidity" => 62.0 + index as f64,
            "rain" => 0.0,
            "quarter_hour" => ((index + 1) % 96) as f64,
            other => panic!("unexpected future feature {other}"),
        })
        .collect()
}

fn source_provenance(
    segment: SegmentKind,
    definitions: &[FeatureDefinition],
    watermark: TimestampMs,
) -> Vec<SourceProvenance> {
    definitions
        .iter()
        .map(|definition| {
            let source_kind = if definition.name() == "quarter_hour" {
                SourceKind::Calendar
            } else if segment == SegmentKind::History {
                SourceKind::History
            } else {
                SourceKind::Covariate
            };
            let provenance = SourceProvenance::new(
                segment,
                definition.name(),
                source_kind,
                Some(definition.name()),
                watermark,
            )
            .expect("provenance is valid");
            if segment == SegmentKind::FutureCovariates && source_kind == SourceKind::Covariate {
                provenance
                    .with_issued_at(watermark)
                    .expect("future covariate issue time is valid")
            } else {
                provenance
            }
        })
        .collect()
}

#[tokio::test]
async fn bundled_load_task_runs_end_to_end_without_external_services() {
    let gateway = EnergyGateway::bundled().expect("energy pack is safe");
    assert_eq!(gateway.pack_summary().data_processing_task_count, 2);
    assert_eq!(gateway.pack_summary().enabled_data_processing_task_count, 0);

    let contract = gateway.load_forecast_contract();
    let configured_task = contract.task().clone();
    let task = configured_task.identity().clone();
    let specification = configured_task
        .forecast_spec()
        .expect("bundled load task is a forecast");
    assert_eq!(specification.cadence_ms(), 900_000);
    assert_eq!(specification.history_steps(), 672);
    assert_eq!(specification.max_horizon_steps(), 288);
    assert_eq!(specification.max_output_age_ms(), 3_600_000);
    assert_eq!(specification.max_missing_ratio(), 0.0);
    assert_eq!(specification.max_input_age_ms(), Some(900_000));
    assert_eq!(specification.max_gap_ms(), Some(1_800_000));
    assert_eq!(specification.allowed_fallbacks(), ["persistence"]);
    let fallback_policy = specification
        .fallback_policy("persistence")
        .expect("persistence has a complete acceptance policy");
    assert_eq!(fallback_policy.version(), "1");
    assert_eq!(fallback_policy.source_feature(), "load");
    assert_eq!(fallback_policy.max_output_age_ms(), 1_800_000);
    assert_eq!(contract.deadline_ms(), 5_000);
    assert_eq!(contract.max_attempts(), 1);
    assert_eq!(contract.max_request_bytes(), 4_194_304);
    assert_eq!(contract.max_input_age_ms(), 900_000);
    assert_eq!(contract.max_gap_ms(), 1_800_000);
    assert_eq!(contract.fallback_max_expires_after_ms(), 1_800_000);
    assert_eq!(contract.maximum_frame_samples(), 4_512);
    assert!(!contract.remote_egress_allowed());

    let expected_features = [
        (FeatureRole::History, "load", "kW"),
        (FeatureRole::History, "temp_avg", "Cel"),
        (FeatureRole::History, "humidity", "%"),
        (FeatureRole::History, "rain", "mm"),
        (FeatureRole::History, "quarter_hour", "1"),
        (FeatureRole::FutureCovariate, "temp_avg", "Cel"),
        (FeatureRole::FutureCovariate, "humidity", "%"),
        (FeatureRole::FutureCovariate, "rain", "mm"),
        (FeatureRole::FutureCovariate, "quarter_hour", "1"),
    ];
    let actual_features: Vec<_> = configured_task
        .features()
        .iter()
        .map(|feature| {
            (
                feature.role(),
                feature.name(),
                feature.unit().expect("all load features are numeric"),
            )
        })
        .collect();
    assert_eq!(actual_features, expected_features);

    let binding = BindingIdentity::new("energy.example-site", 1).expect("binding is valid");
    let cadence_ms = specification.cadence_ms();
    let history_steps = specification.history_steps();
    let as_of = TimestampMs::new(
        cadence_ms
            .checked_mul(u64::try_from(history_steps).expect("history length fits u64"))
            .expect("history span fits u64"),
    );
    let history_definitions: Vec<_> = configured_task
        .features()
        .iter()
        .filter(|feature| feature.role() == FeatureRole::History)
        .cloned()
        .collect();
    let history_timestamps: Vec<_> = (0..history_steps)
        .map(|index| {
            TimestampMs::new(
                cadence_ms
                    .checked_mul(
                        u64::try_from(index + 1).expect("interval-end history index fits u64"),
                    )
                    .expect("history timestamp fits u64"),
            )
        })
        .collect();
    let history_watermark = history_timestamps
        .last()
        .copied()
        .expect("history is non-empty");
    let history_segment = Segment::new(
        history_timestamps,
        history_definitions
            .iter()
            .map(|definition| {
                series(
                    definition.clone(),
                    history_values(definition.name(), history_steps),
                )
            })
            .collect(),
    )
    .expect("history is valid");
    let history = Arc::new(MemoryHistoryQuery::new());
    history
        .replace(
            binding.clone(),
            SourcedSegment::new(
                history_segment,
                source_provenance(
                    SegmentKind::History,
                    &history_definitions,
                    history_watermark,
                ),
            )
            .expect("history is sourced"),
        )
        .expect("history is installed");

    let horizon_steps = 2;
    let future_definitions: Vec<_> = configured_task
        .features()
        .iter()
        .filter(|feature| feature.role() == FeatureRole::FutureCovariate)
        .cloned()
        .collect();
    let future_timestamps: Vec<_> = (1..=horizon_steps)
        .map(|index| {
            TimestampMs::new(
                as_of
                    .get()
                    .checked_add(
                        cadence_ms
                            .checked_mul(u64::try_from(index).expect("horizon index fits u64"))
                            .expect("future offset fits u64"),
                    )
                    .expect("future timestamp fits u64"),
            )
        })
        .collect();
    let future_segment = Segment::new(
        future_timestamps,
        future_definitions
            .iter()
            .map(|definition| {
                series(
                    definition.clone(),
                    future_values(definition.name(), horizon_steps),
                )
            })
            .collect(),
    )
    .expect("future data is valid");
    let covariates = Arc::new(MemoryCovariateSource::new());
    covariates
        .replace(
            binding.clone(),
            SourcedSegment::new(
                future_segment,
                source_provenance(SegmentKind::FutureCovariates, &future_definitions, as_of),
            )
            .expect("future data is sourced"),
        )
        .expect("future data is installed");

    let processor = Arc::new(
        PersistenceForecastProcessor::for_load_contract(contract)
            .expect("local processor is valid"),
    );
    let load_address = aether_sdk::domain::PointAddress::new(
        aether_sdk::domain::InstanceId::new(1),
        aether_sdk::domain::PointKind::Telemetry,
        aether_sdk::domain::PointId::new(1),
    );
    let commissioned_binding = DataProcessingBinding::new(
        binding.clone(),
        vec![PointFeatureBinding::new("load", load_address).expect("point binding is valid")],
    )
    .expect("binding is valid");
    let route = DataProcessingRoute::new(
        configured_task,
        commissioned_binding,
        processor,
        contract.deadline_ms(),
    )
    .expect("route is valid");
    let application = DataProcessingApplication::new(
        vec![route],
        history,
        Some(covariates),
        Arc::new(MemoryLiveState::new()),
        Arc::new(MemoryAuditSink::new()),
        Arc::new(ManualClock::new(TimestampMs::new(as_of.get() + 101))),
        SafetyPolicy,
    )
    .expect("application is valid");

    let result = application
        .process(
            &RequestContext::new(
                "0190aee6-2139-7a87-8448-806f1b843201",
                Actor::new("local:test").with_permission("data_processing.run"),
                false,
                TimestampMs::new(as_of.get() + 100),
            ),
            ProcessTaskRequest::new(
                task,
                binding,
                as_of,
                ProcessingOptions::Forecast(
                    ForecastOptions::new(horizon_steps, vec![]).expect("options are valid"),
                ),
            ),
        )
        .await
        .expect("local persistence forecast succeeds");

    assert_eq!(result.result().status(), ProcessingStatus::Fallback);
    let Some(ProcessingOutput::Forecast(output)) = result.result().output() else {
        panic!("forecast output is required");
    };
    assert_eq!(output.points().len(), horizon_steps);
    let expected_persistence_value = 800.0 + (history_steps - 1) as f64;
    assert!(
        output
            .points()
            .iter()
            .all(|point| point.value() == expected_persistence_value)
    );
    assert_eq!(
        result
            .result()
            .fallback()
            .map(|fallback| fallback.strategy()),
        Some("persistence")
    );
    assert_eq!(
        result
            .result()
            .fallback()
            .map(|fallback| fallback.based_on_data_through()),
        Some(history_watermark)
    );
    let produced_at = result.result().produced_at();
    let expires_at = result
        .result()
        .expires_at()
        .expect("fallback output has a bounded lifetime");
    assert_eq!(
        expires_at.get() - produced_at.get(),
        contract.fallback_max_expires_after_ms()
    );
}

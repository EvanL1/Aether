use aether_domain::{
    BindingIdentity, FeatureDefinition, FeatureRole, FeatureValue, HistoryAggregation,
    HistoryDuplicatePolicy, SampleQuality, Segment, SegmentKind, Series, SourceKind,
    SourceProvenance, TaskIdentity, TimestampMs,
};
use aether_ports::{
    HistoryQuery, HistoryWindow, PortError, PortErrorKind, PortResult, SourcedSegment,
};
use aether_testkit::{assert_history_query_bounded, assert_history_query_provenance};
use async_trait::async_trait;

fn binding() -> BindingIdentity {
    BindingIdentity::new("site-a", 1).expect("binding identity is valid")
}

fn task() -> TaskIdentity {
    TaskIdentity::new("iot.history-test", 1).expect("task is valid")
}

fn history_feature(name: &str, unit: &str) -> FeatureDefinition {
    FeatureDefinition::numeric(name, FeatureRole::History, unit).expect("history feature is valid")
}

fn numeric_series(definition: FeatureDefinition, values: &[f64]) -> Series {
    Series::new(
        definition,
        values
            .iter()
            .map(|value| FeatureValue::number(*value).expect("fixture values are finite"))
            .collect(),
        vec![SampleQuality::Good; values.len()],
    )
    .expect("fixture series is valid")
}

fn sourced_history() -> SourcedSegment {
    let segment = Segment::new(
        vec![TimestampMs::new(1_000), TimestampMs::new(2_000)],
        vec![
            numeric_series(history_feature("load", "kW"), &[10.0, 11.0]),
            numeric_series(history_feature("temperature", "Cel"), &[20.0, 21.0]),
        ],
    )
    .expect("history segment is valid");
    SourcedSegment::new(
        segment,
        vec![
            SourceProvenance::new(
                SegmentKind::History,
                "load",
                SourceKind::History,
                Some("site.load"),
                TimestampMs::new(2_000),
            )
            .expect("load provenance is valid"),
            SourceProvenance::new(
                SegmentKind::History,
                "temperature",
                SourceKind::History,
                Some("site.temperature"),
                TimestampMs::new(2_000),
            )
            .expect("temperature provenance is valid"),
        ],
    )
    .expect("sourced history is valid")
}

fn history_window() -> HistoryWindow {
    HistoryWindow::new(
        task(),
        binding(),
        vec![
            history_feature("load", "kW"),
            history_feature("temperature", "Cel"),
        ],
        TimestampMs::new(1_000),
        TimestampMs::new(3_000),
        2,
        HistoryAggregation::Mean,
        HistoryDuplicatePolicy::Latest,
    )
    .expect("history window is valid")
}

struct BoundedHistoryFixture {
    response: SourcedSegment,
}

#[async_trait]
impl HistoryQuery for BoundedHistoryFixture {
    async fn query(&self, window: HistoryWindow) -> PortResult<SourcedSegment> {
        if self.response.segment().sample_count() > window.max_samples() {
            return Err(PortError::new(
                PortErrorKind::Rejected,
                "fixture response exceeds sample bound",
            ));
        }
        Ok(self.response.clone())
    }
}

struct ReorderedHistoryFixture {
    response: SourcedSegment,
}

#[async_trait]
impl HistoryQuery for ReorderedHistoryFixture {
    async fn query(&self, _window: HistoryWindow) -> PortResult<SourcedSegment> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn history_conformance_checks_bounds_feature_order_and_provenance() {
    let expected = sourced_history();
    let query = BoundedHistoryFixture {
        response: expected.clone(),
    };

    assert_history_query_bounded(&query, history_window(), expected.clone())
        .await
        .expect("bounded response satisfies conformance");
    assert_history_query_provenance(&query, history_window(), expected.provenance())
        .await
        .expect("history provenance satisfies conformance");
}

#[tokio::test]
async fn history_conformance_rejects_response_feature_reordering() {
    let source = sourced_history();
    let reversed_segment = Segment::new(
        source.segment().timestamps().to_vec(),
        vec![
            source.segment().series()[1].clone(),
            source.segment().series()[0].clone(),
        ],
    )
    .expect("reordered segment remains structurally valid");
    let reordered = SourcedSegment::new(reversed_segment, source.provenance().to_vec())
        .expect("reordered response remains structurally valid");
    let query = ReorderedHistoryFixture {
        response: reordered,
    };

    let error = assert_history_query_bounded(&query, history_window(), sourced_history())
        .await
        .expect_err("logical feature order is part of the port contract");

    assert_eq!(error.kind(), PortErrorKind::InvalidData);
}

#[test]
fn sourced_history_rejects_non_history_provenance_before_conformance() {
    let source = sourced_history();
    let invalid_provenance = source
        .provenance()
        .iter()
        .map(|entry| {
            SourceProvenance::new(
                SegmentKind::FutureCovariates,
                entry.feature(),
                SourceKind::Covariate,
                entry.source_ref(),
                entry.watermark(),
            )
            .expect("wrong-kind provenance is structurally valid")
        })
        .collect::<Vec<_>>();
    let error = SourcedSegment::new(source.segment().clone(), invalid_provenance)
        .expect_err("the source boundary rejects the wrong semantic segment kind");

    assert_eq!(error.kind(), PortErrorKind::InvalidData);
}

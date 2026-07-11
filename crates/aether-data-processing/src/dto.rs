use std::collections::BTreeMap;

use aether_domain::{DataProcessingRequest, DerivedData, ProcessingResult};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::codec::{
    CodecError, derived_data_to_wire, request_from_wire, request_to_wire, result_from_wire,
    result_to_wire,
};

/// Validated RFC 3339/JSON representation of a processor request.
///
/// Deserialization performs strict schema, time, digest, state, and domain
/// validation. Use [`Self::into_domain`] to cross into the domain layer.
#[derive(Debug, Clone, PartialEq)]
pub struct DataProcessingRequestDto(DataProcessingRequest);

impl DataProcessingRequestDto {
    /// Consumes the DTO and returns the validated domain request.
    #[must_use]
    pub fn into_domain(self) -> DataProcessingRequest {
        self.0
    }

    /// Borrows the validated domain request.
    #[must_use]
    pub const fn as_domain(&self) -> &DataProcessingRequest {
        &self.0
    }
}

impl TryFrom<&DataProcessingRequest> for DataProcessingRequestDto {
    type Error = CodecError;

    fn try_from(request: &DataProcessingRequest) -> Result<Self, Self::Error> {
        let wire = request_to_wire(request)?;
        Ok(Self(request_from_wire(wire)?))
    }
}

impl Serialize for DataProcessingRequestDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        request_to_wire(&self.0)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DataProcessingRequestDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        reject_explicit_nulls(&value, false).map_err(D::Error::custom)?;
        let wire = serde_json::from_value::<RequestWire>(value).map_err(D::Error::custom)?;
        request_from_wire(wire).map(Self).map_err(D::Error::custom)
    }
}

/// Validated RFC 3339/JSON representation of an untrusted processor result.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessingResultDto(ProcessingResult);

impl ProcessingResultDto {
    /// Consumes the DTO and returns the validated domain result.
    #[must_use]
    pub fn into_domain(self) -> ProcessingResult {
        self.0
    }

    /// Borrows the validated domain result.
    #[must_use]
    pub const fn as_domain(&self) -> &ProcessingResult {
        &self.0
    }
}

impl TryFrom<&ProcessingResult> for ProcessingResultDto {
    type Error = CodecError;

    fn try_from(result: &ProcessingResult) -> Result<Self, Self::Error> {
        let wire = result_to_wire(result)?;
        Ok(Self(result_from_wire(wire)?))
    }
}

impl Serialize for ProcessingResultDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        result_to_wire(&self.0)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProcessingResultDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        reject_explicit_nulls(&value, false).map_err(D::Error::custom)?;
        let wire = serde_json::from_value::<ResultWire>(value).map_err(D::Error::custom)?;
        result_from_wire(wire).map(Self).map_err(D::Error::custom)
    }
}

/// Strict JSON representation of Aether-accepted derived data.
///
/// This type intentionally implements encoding only. Processor JSON must first
/// cross the untrusted [`ProcessingResultDto`] boundary and pass application
/// validation before the domain can create [`DerivedData`].
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedDataDto(DerivedData);

impl DerivedDataDto {
    /// Borrows the accepted domain value represented by this DTO.
    #[must_use]
    pub const fn as_domain(&self) -> &DerivedData {
        &self.0
    }
}

impl TryFrom<&DerivedData> for DerivedDataDto {
    type Error = CodecError;

    fn try_from(derived: &DerivedData) -> Result<Self, Self::Error> {
        derived_data_to_wire(derived)?;
        Ok(Self(derived.clone()))
    }
}

impl Serialize for DerivedDataDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        derived_data_to_wire(&self.0)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

fn reject_explicit_nulls(
    value: &serde_json::Value,
    null_array_elements_allowed: bool,
) -> Result<(), &'static str> {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if value.is_null() {
                    if name != "value" {
                        return Err("optional Data Processing fields must be omitted, not null");
                    }
                } else {
                    reject_explicit_nulls(value, name == "values")?;
                }
            }
        },
        serde_json::Value::Array(values) => {
            for value in values {
                if value.is_null() && !null_array_elements_allowed {
                    return Err("null is allowed only for explicitly missing feature values");
                }
                if !value.is_null() {
                    reject_explicit_nulls(value, false)?;
                }
            }
        },
        serde_json::Value::Null if !null_array_elements_allowed => {
            return Err("optional Data Processing fields must be omitted, not null");
        },
        _ => {},
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestWire {
    pub schema: String,
    pub request_id: String,
    pub submitted_at: String,
    pub deadline: String,
    pub task: TaskWire,
    pub binding: BindingWire,
    pub processor_contract: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactSelectorWire>,
    pub frame: FrameWire,
    pub options: OptionsWire,
    pub input_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskWire {
    pub id: String,
    pub revision: u32,
    pub kind: TaskKindWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskKindWire {
    Forecast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingWire {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactSelectorWire {
    pub kind: String,
    pub family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameWire {
    pub schema: String,
    pub as_of: String,
    pub cadence_seconds: u64,
    pub history: SegmentWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub future_covariates: Option<SegmentWire>,
    #[serde(default)]
    pub static_features: BTreeMap<String, StaticFeatureWire>,
    pub quality: FrameQualityWire,
    pub provenance: Vec<SourceProvenanceWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentWire {
    pub timestamps: Vec<String>,
    pub features: BTreeMap<String, SeriesWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeriesWire {
    pub value_type: FeatureValueTypeWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub values: Vec<ScalarWire>,
    pub quality: Vec<SampleQualityWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StaticFeatureWire {
    pub value_type: FeatureValueTypeWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub value: ScalarWire,
    pub quality: SampleQualityWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FeatureValueTypeWire {
    Number,
    String,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum ScalarWire {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SampleQualityWire {
    Good,
    Uncertain,
    Substituted,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameQualityWire {
    pub input_watermark: String,
    pub missing_ratio: f64,
    pub max_gap_seconds: u64,
    pub live_tail_included: bool,
    pub substituted_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceProvenanceWire {
    pub segment: SegmentKindWire,
    pub feature: String,
    pub source_kind: SourceKindWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub watermark: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SegmentKindWire {
    History,
    FutureCovariates,
    StaticFeatures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKindWire {
    History,
    Live,
    HistoryAndLive,
    Covariate,
    Calendar,
    Constant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OptionsWire {
    Forecast {
        horizon_steps: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quantiles: Option<Vec<f64>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResultWire {
    pub schema: String,
    pub request_id: String,
    pub task: TaskWire,
    pub binding: BindingWire,
    pub input_digest: String,
    pub status: ProcessingStatusWire,
    pub issued_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub input_watermark: String,
    pub processor: ProcessorWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactProvenanceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<ForecastOutputWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<UnavailableWire>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProcessingStatusWire {
    Produced,
    Fallback,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessorWire {
    pub id: String,
    pub version: String,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactProvenanceWire {
    pub kind: String,
    pub family: String,
    pub version: String,
    pub artifact_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForecastOutputWire {
    pub schema: String,
    pub kind: TaskKindWire,
    pub target: String,
    pub unit: String,
    pub sign_convention: String,
    pub cadence_seconds: u64,
    pub timestamp_semantics: TimestampSemanticsWire,
    pub points: Vec<ForecastPointWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TimestampSemanticsWire {
    IntervalStart,
    IntervalEnd,
    Instant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForecastPointWire {
    pub timestamp: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantiles: Option<Vec<ForecastQuantileWire>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForecastQuantileWire {
    pub probability: f64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FallbackWire {
    pub strategy: String,
    pub strategy_version: String,
    pub reason_code: String,
    pub source_feature: String,
    pub based_on_data_through: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnavailableWire {
    pub reason_code: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DerivedDataWire {
    pub schema: String,
    pub result_id: String,
    pub request_id: String,
    pub task: TaskWire,
    pub binding: BindingWire,
    pub accepted_at: String,
    pub expires_at: String,
    pub input_digest: String,
    pub processing_status: AcceptedProcessingStatusWire,
    pub processor: ProcessorWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactProvenanceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackWire>,
    pub warnings: Vec<String>,
    pub quality: AcceptedFrameQualityWire,
    pub data: ForecastOutputWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AcceptedProcessingStatusWire {
    Produced,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptedFrameQualityWire {
    pub input_watermark: String,
    pub missing_ratio: f64,
    pub max_gap_seconds: u64,
    pub live_tail_included: bool,
    pub substituted_samples: u64,
    pub fallback_used: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DigestBasisWire<'a> {
    pub task: &'a TaskWire,
    pub binding: &'a BindingWire,
    pub processor_contract: &'a str,
    pub artifact: Option<&'a ArtifactSelectorWire>,
    pub frame: &'a FrameWire,
    pub options: &'a OptionsWire,
}

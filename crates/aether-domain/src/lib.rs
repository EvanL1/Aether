//! Industry-neutral domain types for the Aether edge kernel.

#![no_std]

extern crate alloc;

mod command;
mod data_processing;
mod error;
mod identity;
mod point;

pub use command::{CommandConstraints, ControlCommand, DEFAULT_COMMAND_TTL_MS};
pub use data_processing::{
    ArtifactProvenance, ArtifactSelector, BindingIdentity, DataProcessingRequest,
    DataProcessingTask, DerivedData, FallbackInfo, FallbackPolicy, FeatureDefinition, FeatureRole,
    FeatureValue, FeatureValueType, ForecastOptions, ForecastOutput, ForecastPoint,
    ForecastQuantile, ForecastTarget, ForecastTaskSpec, FrameQuality, HistoryAggregation,
    HistoryDuplicatePolicy, HistoryFeaturePolicy, NumericFeatureConstraints, ProcessTaskRequest,
    ProcessingFrame, ProcessingOptions, ProcessingOutput, ProcessingResult, ProcessingStatus,
    ProcessorProvenance, SampleQuality, Segment, SegmentKind, Series, SourceKind, SourceProvenance,
    StaticFeature, TaskIdentity, TaskKind, UnavailableInfo, is_semantic_source_ref,
    maximum_observation_gap,
};
pub use error::DomainError;
pub use identity::{CommandId, InstanceId, PointId, TimestampMs};
pub use point::{PointAddress, PointKind, PointQuality, PointSample};

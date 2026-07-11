//! Transport-neutral Aether use cases and safety policy.

mod capability;
mod context;
mod control;
mod data_processing;
mod edge;
mod error;
mod outbox_forwarder;
mod policy;

pub use aether_domain::DEFAULT_COMMAND_TTL_MS;
pub use capability::{
    AuditPolicy, CapabilityDescriptor, ConfirmationPolicy, OperationKind, PROCESS_DATA_CAPABILITY,
    PROCESSOR_HEALTH_CAPABILITY, READ_POINT_CAPABILITY, RiskLevel, TASKS_LIST_CAPABILITY,
    WRITE_POINT_CAPABILITY, capability_catalog,
};
pub use context::{Actor, RequestContext};
pub use control::ControlApplication;
pub use data_processing::{
    DATA_PROCESSING_AUDIT_FINALIZATION_TIMEOUT_MS, DataProcessingApplication,
    DataProcessingBinding, DataProcessingRoute, DataProcessingTaskSummary, PointFeatureBinding,
    ProcessorHealthSummary,
};
pub use edge::EdgeApplication;
pub use error::ApplicationError;
pub use outbox_forwarder::{DrainReport, OutboxForwarder};
pub use policy::SafetyPolicy;

//! Capability-oriented ports implemented by Aether extensions.

mod audit;
mod clock;
mod control;
mod data_processing;
mod error;
mod history;
mod live_state;
mod mirror;
mod outbox;
mod uplink;

pub use audit::{AuditOutcome, AuditRecord, AuditSink};
pub use clock::Clock;
pub use control::{CommandDispatcher, CommandReceipt};
pub use data_processing::{
    CovariateSource, CovariateWindow, DataBoundary, DataProcessor, DataProcessorDescriptor,
    HistoryQuery, HistoryWindow, ProcessorHealth, SourcedSegment,
};
pub use error::{PortError, PortErrorKind, PortResult};
pub use history::HistorySink;
pub use live_state::{LiveState, LiveStateWriter};
pub use mirror::StateMirror;
pub use outbox::{DurableOutbox, OutboxEntry, OutboxId, OutboxMessage};
pub use uplink::UplinkPublisher;

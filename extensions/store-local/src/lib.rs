//! Local adapters that require no external service.

mod audit;
mod clock;
mod data_processing;
mod file_outbox;
mod history;
mod live_state;
mod outbox;
mod snapshot_covariates;

use aether_ports::{PortError, PortErrorKind};

pub use audit::MemoryAuditSink;
#[cfg(feature = "sqlite-audit")]
pub use audit::SqliteAuditSink;
pub use clock::{ManualClock, SystemClock};
pub use data_processing::{MemoryCovariateSource, MemoryHistoryQuery};
pub use file_outbox::FileOutbox;
pub use history::MemoryHistorySink;
pub use live_state::MemoryLiveState;
pub use outbox::MemoryOutbox;
pub use snapshot_covariates::{SnapshotCovariateLimits, SnapshotCovariateSource};

fn lock_error(resource: &str) -> PortError {
    PortError::new(
        PortErrorKind::Permanent,
        format!("{resource} lock was poisoned"),
    )
}

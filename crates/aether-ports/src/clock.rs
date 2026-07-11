//! Trusted wall-clock capability used at application acceptance boundaries.

use aether_domain::TimestampMs;

use crate::PortResult;

/// Trusted time source selected by a composition root.
pub trait Clock: Send + Sync + 'static {
    /// Returns the current UTC Unix timestamp in milliseconds.
    fn now(&self) -> PortResult<TimestampMs>;
}

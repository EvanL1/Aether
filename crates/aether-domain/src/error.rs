//! Domain validation failures.

use core::fmt;

use crate::PointKind;

/// Error returned when a domain invariant would be violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    /// An identity, contract, feature, target, reason, or digest was empty.
    EmptyIdentifier,
    /// A versioned identity used revision zero.
    ZeroRevision,
    /// A required collection was empty.
    EmptyCollection,
    /// A task or segment declared the same feature more than once.
    DuplicateFeature,
    /// A processing number was NaN or infinite.
    NonFiniteProcessingValue,
    /// A feature value did not match its declared type.
    FeatureTypeMismatch,
    /// Parallel processing arrays had different lengths.
    ArrayLengthMismatch,
    /// A missing value and its quality marker disagreed.
    InvalidSampleQuality,
    /// Timestamps were duplicated or out of order.
    TimestampsNotStrictlyIncreasing,
    /// Frame quality values were outside their valid range.
    InvalidFrameQuality,
    /// A processing cutoff, source window, deadline, or expiry was invalid.
    InvalidProcessingWindow,
    /// A quantile probability or ordering was invalid.
    InvalidQuantile,
    /// A processing result combined status-specific fields illegally.
    InvalidProcessingState,
    /// A control command targeted a read-only point.
    PointNotWritable(PointKind),
    /// A command value was NaN or infinite.
    NonFiniteCommandValue,
    /// The command deadline was not later than its issue time.
    InvalidCommandWindow,
    /// A command reached a dispatch boundary at or after its deadline.
    CommandExpired,
    /// A point's configured bounds or step are internally inconsistent.
    InvalidCommandConstraints,
    /// A command value is outside the point's inclusive range.
    CommandValueOutOfRange,
    /// A command value is not aligned to the point's configured step.
    CommandValueOffStep,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentifier => formatter.write_str("identifier must not be empty"),
            Self::ZeroRevision => formatter.write_str("revision must be greater than zero"),
            Self::EmptyCollection => formatter.write_str("required collection must not be empty"),
            Self::DuplicateFeature => formatter.write_str("feature names must be unique"),
            Self::NonFiniteProcessingValue => {
                formatter.write_str("processing values must be finite")
            },
            Self::FeatureTypeMismatch => {
                formatter.write_str("feature value does not match its declared type")
            },
            Self::ArrayLengthMismatch => {
                formatter.write_str("processing arrays must have matching lengths")
            },
            Self::InvalidSampleQuality => {
                formatter.write_str("sample value and quality are inconsistent")
            },
            Self::TimestampsNotStrictlyIncreasing => {
                formatter.write_str("timestamps must be strictly increasing")
            },
            Self::InvalidFrameQuality => formatter.write_str("frame quality is invalid"),
            Self::InvalidProcessingWindow => {
                formatter.write_str("processing time window is invalid")
            },
            Self::InvalidQuantile => formatter.write_str("forecast quantile is invalid"),
            Self::InvalidProcessingState => {
                formatter.write_str("processing status fields are inconsistent")
            },
            Self::PointNotWritable(kind) => {
                write!(formatter, "point kind {kind:?} is not writable")
            },
            Self::NonFiniteCommandValue => formatter.write_str("command value must be finite"),
            Self::InvalidCommandWindow => {
                formatter.write_str("command expiry must be later than its issue time")
            },
            Self::CommandExpired => formatter.write_str("command has expired"),
            Self::InvalidCommandConstraints => {
                formatter.write_str("command constraints are invalid")
            },
            Self::CommandValueOutOfRange => {
                formatter.write_str("command value is outside the allowed range")
            },
            Self::CommandValueOffStep => {
                formatter.write_str("command value is not aligned to the allowed step")
            },
        }
    }
}

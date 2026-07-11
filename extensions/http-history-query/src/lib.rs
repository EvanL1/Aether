//! Bounded, loopback-only adapter from Aether's logical [`HistoryQuery`]
//! port to the existing `aether-history` batch API.

mod adapter;
mod config;

pub use adapter::HttpHistoryQuery;
pub use config::{
    CalendarFeature, HistoryFeatureRoute, HistoryFeatureSource, HttpHistoryQueryConfig,
};

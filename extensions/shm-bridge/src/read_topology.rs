//! Coherent read-side publication of the point and channel-health SHM planes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_dataplane::AuthorityReadGuard;
use aether_ports::{PortError, PortErrorKind, PortResult};
use arc_swap::ArcSwap;

use crate::managed::map_dataplane_error;
use crate::{
    ChannelHealthManifest, ChannelPointManifest, ReconnectingSlotSource, ShmChannelHealthReader,
    ShmClientConfig,
};

/// One immutable point/health topology generation for read-only consumers.
///
/// The generation deliberately contains both planes. A consumer may retain
/// the returned `Arc` for a whole query or scheduler pass and cannot observe a
/// point manifest from one publication with a health manifest from another.
pub struct ShmReadTopologyGeneration {
    point_path: PathBuf,
    health_path: PathBuf,
    point_source: Arc<ReconnectingSlotSource>,
    point_manifest: Arc<ChannelPointManifest>,
    channel_health: Arc<ShmChannelHealthReader>,
    health_manifest: Arc<ChannelHealthManifest>,
}

impl std::fmt::Debug for ShmReadTopologyGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShmReadTopologyGeneration")
            .field("point_layout_hash", &self.point_manifest.layout_hash())
            .field("point_slot_count", &self.point_manifest.slot_count())
            .field("health_layout_hash", &self.health_manifest.layout_hash())
            .field("health_slot_count", &self.health_manifest.slot_count())
            .finish()
    }
}

impl ShmReadTopologyGeneration {
    /// Builds a lazy generation so a service can start before IO.
    ///
    /// This validates the composition-provided hashes but does not open either
    /// SHM path. Reads remain retryably unavailable until IO publishes them.
    pub fn new_lazy(
        point_config: ShmClientConfig,
        health_config: ShmClientConfig,
        point_manifest: Arc<ChannelPointManifest>,
        health_manifest: Arc<ChannelHealthManifest>,
    ) -> PortResult<Self> {
        if point_config.path() == health_config.path() {
            return Err(PortError::new(
                PortErrorKind::InvalidData,
                "point and channel-health SHM paths must be distinct",
            ));
        }
        validate_config_hash(
            "point",
            point_config.expected_layout_hash(),
            point_manifest.layout_hash(),
        )?;
        validate_config_hash(
            "channel-health",
            health_config.expected_layout_hash(),
            health_manifest.layout_hash(),
        )?;
        Ok(Self {
            point_path: point_config.path().to_path_buf(),
            health_path: health_config.path().to_path_buf(),
            point_source: Arc::new(ReconnectingSlotSource::new(point_config)),
            point_manifest,
            channel_health: Arc::new(ShmChannelHealthReader::new(
                health_config,
                Arc::clone(&health_manifest),
            )),
            health_manifest,
        })
    }

    /// Opens and validates both physical planes before returning a candidate.
    pub fn open(
        point_config: ShmClientConfig,
        health_config: ShmClientConfig,
        point_manifest: Arc<ChannelPointManifest>,
        health_manifest: Arc<ChannelHealthManifest>,
    ) -> PortResult<Self> {
        let generation =
            Self::new_lazy(point_config, health_config, point_manifest, health_manifest)?;
        generation.validate_layouts()?;
        Ok(generation)
    }

    /// Revalidates both layouts without requiring a fresh heartbeat.
    pub fn validate_layouts(&self) -> PortResult<()> {
        self.with_validated_authority(|| ())
    }

    /// Runs one publication while both canonical planes are validation-locked.
    ///
    /// IO needs exclusive leases to invalidate and replace either generation,
    /// so a service-local `ArcSwap` performed in this closure cannot race the
    /// final validation and publish a mapping that was already superseded.
    pub fn with_validated_authority<T>(&self, publish: impl FnOnce() -> T) -> PortResult<T> {
        let (_first, _second) = acquire_authority_pair(&self.point_path, &self.health_path)?;
        self.point_source
            .validate_layout(self.point_manifest.slot_count())?;
        self.channel_health.validate_layout()?;
        Ok(publish())
    }

    /// Returns the point-plane source paired with this generation.
    #[must_use]
    pub fn point_source(&self) -> &Arc<ReconnectingSlotSource> {
        &self.point_source
    }

    /// Returns the point manifest paired with this generation.
    #[must_use]
    pub fn point_manifest(&self) -> &Arc<ChannelPointManifest> {
        &self.point_manifest
    }

    /// Returns the channel-health source paired with this generation.
    #[must_use]
    pub fn channel_health(&self) -> &Arc<ShmChannelHealthReader> {
        &self.channel_health
    }

    /// Returns the health manifest paired with this generation.
    #[must_use]
    pub fn health_manifest(&self) -> &Arc<ChannelHealthManifest> {
        &self.health_manifest
    }
}

/// Atomically replaceable read-side topology generation.
pub struct ShmReadTopologyHandle {
    current: ArcSwap<ShmReadTopologyGeneration>,
}

impl ShmReadTopologyHandle {
    /// Creates a handle, typically with a lazy startup generation.
    #[must_use]
    pub fn new(initial: Arc<ShmReadTopologyGeneration>) -> Self {
        Self {
            current: ArcSwap::new(initial),
        }
    }

    /// Pins one coherent generation for an entire logical read operation.
    #[must_use]
    pub fn load(&self) -> Arc<ShmReadTopologyGeneration> {
        self.current.load_full()
    }

    /// Validates and publishes a complete replacement generation.
    pub fn publish(&self, candidate: Arc<ShmReadTopologyGeneration>) -> PortResult<()> {
        candidate.with_validated_authority(|| self.current.store(candidate.clone()))
    }
}

fn acquire_authority_pair(
    point_path: &Path,
    health_path: &Path,
) -> PortResult<(AuthorityReadGuard, AuthorityReadGuard)> {
    let (first_path, second_path) = if point_path <= health_path {
        (point_path, health_path)
    } else {
        (health_path, point_path)
    };
    let first = AuthorityReadGuard::acquire(first_path).map_err(map_dataplane_error)?;
    let second = AuthorityReadGuard::acquire(second_path).map_err(map_dataplane_error)?;
    Ok((first, second))
}

fn validate_config_hash(label: &str, configured: u64, manifest: u64) -> PortResult<()> {
    if configured == manifest {
        return Ok(());
    }
    Err(PortError::new(
        PortErrorKind::InvalidData,
        format!(
            "{label} SHM client hash 0x{configured:016x} does not match manifest hash 0x{manifest:016x}"
        ),
    ))
}

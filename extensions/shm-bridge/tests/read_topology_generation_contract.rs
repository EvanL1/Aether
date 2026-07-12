use std::sync::Arc;

use aether_ports::PortErrorKind;
use aether_shm_bridge::{
    ChannelHealthManifest, ChannelPointManifest, ShmChannelHealthWriterHandle, ShmClientConfig,
    ShmReadTopologyGeneration, ShmReadTopologyHandle, ShmRuntimeConfig, ShmWriterHandle,
    SlotSource,
};

fn point_manifest(entries: &[(u32, [u32; 4])]) -> Arc<ChannelPointManifest> {
    Arc::new(ChannelPointManifest::from_entries(entries.iter().copied()))
}

fn health_manifest(channel_ids: &[u32]) -> Arc<ChannelHealthManifest> {
    Arc::new(ChannelHealthManifest::from_channel_ids(
        channel_ids.iter().copied(),
    ))
}

#[test]
fn validated_reader_generation_opens_only_when_both_planes_match() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let point_path = directory.path().join("live.shm");
    let health_path = directory.path().join("health.shm");
    let points = point_manifest(&[(7, [1, 1, 0, 0])]);
    let health = health_manifest(&[7]);

    let point_writer = ShmWriterHandle::create_published(
        ShmRuntimeConfig::new(&point_path, 32),
        Arc::clone(&points),
        None,
    )
    .expect("publish point generation");
    let health_writer = ShmChannelHealthWriterHandle::create(&health_path, Arc::clone(&health))
        .expect("publish health generation");
    let now_ms = aether_shm_bridge::timestamp_ms();
    point_writer
        .generation()
        .expect("point generation")
        .acquisition_writer()
        .update_heartbeat(now_ms);
    health_writer
        .update_heartbeat(now_ms)
        .expect("publish health heartbeat");

    let generation = ShmReadTopologyGeneration::open(
        ShmClientConfig::new(&point_path, points.layout_hash()),
        ShmClientConfig::new(&health_path, health.layout_hash()),
        Arc::clone(&points),
        Arc::clone(&health),
    )
    .expect("open coherent read generation");

    assert_eq!(
        generation.point_manifest().layout_hash(),
        points.layout_hash()
    );
    assert_eq!(
        generation.health_manifest().layout_hash(),
        health.layout_hash()
    );
    assert_eq!(generation.point_source().slot_count().unwrap(), 2);
    assert!(
        generation
            .channel_health()
            .read_channel(7)
            .unwrap()
            .is_none()
    );
}

#[test]
fn partial_dual_plane_publication_is_never_accepted_as_a_reader_generation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let point_path = directory.path().join("live.shm");
    let health_path = directory.path().join("health.shm");
    let old_points = point_manifest(&[(7, [1, 0, 0, 0])]);
    let old_health = health_manifest(&[7]);
    let new_points = point_manifest(&[(7, [1, 0, 0, 0]), (9, [1, 0, 0, 0])]);
    let new_health = health_manifest(&[7, 9]);

    let point_writer =
        ShmWriterHandle::create_published(ShmRuntimeConfig::new(&point_path, 32), old_points, None)
            .expect("publish old point generation");
    let _health_writer = ShmChannelHealthWriterHandle::create(&health_path, old_health)
        .expect("publish old health generation");
    point_writer
        .rebuild(Arc::clone(&new_points))
        .expect("publish only the new point plane");

    let error = ShmReadTopologyGeneration::open(
        ShmClientConfig::new(&point_path, new_points.layout_hash()),
        ShmClientConfig::new(&health_path, new_health.layout_hash()),
        new_points,
        new_health,
    )
    .expect_err("mixed point/health generations must fail closed");

    assert_eq!(error.kind(), PortErrorKind::Conflict);
    assert!(error.is_retryable());
}

#[test]
fn handle_retains_its_previous_generation_when_candidate_validation_fails() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let point_path = directory.path().join("live.shm");
    let health_path = directory.path().join("health.shm");
    let old_points = point_manifest(&[(7, [1, 0, 0, 0])]);
    let old_health = health_manifest(&[7]);
    let new_points = point_manifest(&[(7, [1, 0, 0, 0]), (9, [1, 0, 0, 0])]);
    let new_health = health_manifest(&[7, 9]);
    let point_writer = ShmWriterHandle::create_published(
        ShmRuntimeConfig::new(&point_path, 32),
        Arc::clone(&old_points),
        None,
    )
    .expect("publish old point generation");
    let _health_writer =
        ShmChannelHealthWriterHandle::create(&health_path, Arc::clone(&old_health))
            .expect("publish old health generation");
    let initial = Arc::new(
        ShmReadTopologyGeneration::open(
            ShmClientConfig::new(&point_path, old_points.layout_hash()),
            ShmClientConfig::new(&health_path, old_health.layout_hash()),
            Arc::clone(&old_points),
            Arc::clone(&old_health),
        )
        .expect("open initial topology"),
    );
    let handle = ShmReadTopologyHandle::new(initial);

    point_writer
        .rebuild(Arc::clone(&new_points))
        .expect("publish only the new point plane");
    let candidate = Arc::new(
        ShmReadTopologyGeneration::new_lazy(
            ShmClientConfig::new(&point_path, new_points.layout_hash()),
            ShmClientConfig::new(&health_path, new_health.layout_hash()),
            new_points,
            new_health,
        )
        .expect("compose replacement candidate"),
    );

    let error = handle
        .publish(candidate)
        .expect_err("partial physical publication must not advance the handle");

    assert_eq!(error.kind(), PortErrorKind::Conflict);
    assert_eq!(
        handle.load().point_manifest().layout_hash(),
        old_points.layout_hash()
    );
    assert_eq!(
        handle.load().health_manifest().layout_hash(),
        old_health.layout_hash()
    );
}

#[test]
fn composition_hash_mismatch_is_permanent_but_physical_lag_is_retryable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let point_path = directory.path().join("live.shm");
    let health_path = directory.path().join("health.shm");
    let points = point_manifest(&[(7, [1, 0, 0, 0])]);
    let health = health_manifest(&[7]);

    let error = ShmReadTopologyGeneration::new_lazy(
        ShmClientConfig::new(&point_path, points.layout_hash().wrapping_add(1)),
        ShmClientConfig::new(&health_path, health.layout_hash()),
        points,
        health,
    )
    .expect_err("composition-provided config and manifest must agree");

    assert_eq!(error.kind(), PortErrorKind::InvalidData);
    assert!(!error.is_retryable());
}

#[test]
fn lazy_reader_generation_keeps_service_startup_independent_from_io() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let point_path = directory.path().join("missing-live.shm");
    let health_path = directory.path().join("missing-health.shm");
    let points = point_manifest(&[(7, [1, 0, 0, 0])]);
    let health = health_manifest(&[7]);

    let generation = ShmReadTopologyGeneration::new_lazy(
        ShmClientConfig::new(&point_path, points.layout_hash()),
        ShmClientConfig::new(&health_path, health.layout_hash()),
        points,
        health,
    )
    .expect("compose lazy generation");

    let error = generation
        .point_source()
        .slot_count()
        .expect_err("missing io writer is a retryable read-time condition");
    assert_eq!(error.kind(), PortErrorKind::Unavailable);
    assert!(error.is_retryable());
}

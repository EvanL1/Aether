//! Automation composition adapter for authoritative rule live-state reads.

use std::sync::Arc;

use aether_domain::PointKind;
use aether_ports::CommandTopologyFence;
use aether_routing::RoutingCache;
use aether_rules::{RuleExecutionContext, RuleLiveState};
use aether_shm_bridge::ShmChannelReaderHandle;

/// Rule live-state adapter backed by the current SHM generation and routing
/// snapshot owned by the Automation service composition root.
pub struct ShmRuleLiveState {
    source: RuleLiveSource,
}

enum RuleLiveSource {
    Runtime(Arc<crate::infra::runtime_topology::AutomationTopologyHandle>),
    Legacy {
        reader: Arc<ShmChannelReaderHandle>,
        routing_cache: Arc<RoutingCache>,
    },
}

impl ShmRuleLiveState {
    /// Creates a read-only rule input over the current SHM reader and routing snapshot.
    #[must_use]
    pub fn new(reader: Arc<ShmChannelReaderHandle>, routing_cache: Arc<RoutingCache>) -> Self {
        Self {
            source: RuleLiveSource::Legacy {
                reader,
                routing_cache,
            },
        }
    }

    /// Creates a production adapter over the atomically replaceable complete topology.
    #[must_use]
    pub fn from_topology(
        topology: Arc<crate::infra::runtime_topology::AutomationTopologyHandle>,
    ) -> Self {
        Self {
            source: RuleLiveSource::Runtime(topology),
        }
    }
}

impl RuleLiveState for ShmRuleLiveState {
    fn begin_execution(&self) -> RuleExecutionContext {
        match &self.source {
            RuleLiveSource::Runtime(topology) => RuleExecutionContext::topology_fenced(
                CommandTopologyFence::new(topology.load().sequence()),
            ),
            RuleLiveSource::Legacy { .. } => RuleExecutionContext::unfenced(),
        }
    }

    fn get_instance(
        &self,
        instance_id: u32,
        instance_type: u8,
        point_id: u32,
    ) -> Option<(f64, u64)> {
        match &self.source {
            RuleLiveSource::Runtime(topology) => topology
                .load()
                .read_instance_point(instance_id, instance_type != 0, point_id)
                .ok()
                .flatten(),
            RuleLiveSource::Legacy {
                reader,
                routing_cache,
            } => read_instance_point(reader, routing_cache, instance_id, instance_type, point_id),
        }
    }

    fn get_instance_for_execution(
        &self,
        execution: RuleExecutionContext,
        instance_id: u32,
        instance_type: u8,
        point_id: u32,
    ) -> Option<(f64, u64)> {
        match &self.source {
            RuleLiveSource::Runtime(topology) => {
                let fence = execution.command_topology_fence()?;
                let generation = topology.load();
                if generation.sequence() != fence.expected_sequence() {
                    return None;
                }
                generation
                    .read_instance_point(instance_id, instance_type != 0, point_id)
                    .ok()
                    .flatten()
            },
            RuleLiveSource::Legacy {
                reader,
                routing_cache,
            } => read_instance_point(reader, routing_cache, instance_id, instance_type, point_id),
        }
    }
}

/// Resolves one logical instance point through the service-owned routing cache
/// and reads the resulting physical channel address from SHM.
pub(crate) fn read_instance_point(
    reader: &ShmChannelReaderHandle,
    routing_cache: &RoutingCache,
    instance_id: u32,
    instance_type: u8,
    point_id: u32,
) -> Option<(f64, u64)> {
    let (channel_id, kind, channel_point_id) = if instance_type == 0 {
        let (channel_id, point_type, channel_point_id) =
            routing_cache.lookup_c2m_reverse(instance_id, point_id)?;
        let kind = match point_type {
            aether_model::PointType::Telemetry => PointKind::Telemetry,
            aether_model::PointType::Signal => PointKind::Status,
            aether_model::PointType::Control | aether_model::PointType::Adjustment => return None,
        };
        (channel_id, kind, channel_point_id)
    } else {
        let target = routing_cache
            .lookup_m2c_by_parts(instance_id, aether_model::PointType::Control, point_id)
            .or_else(|| {
                routing_cache.lookup_m2c_by_parts(
                    instance_id,
                    aether_model::PointType::Adjustment,
                    point_id,
                )
            })?;
        let kind = match target.point_type {
            aether_model::PointType::Control => PointKind::Command,
            aether_model::PointType::Adjustment => PointKind::Action,
            aether_model::PointType::Telemetry | aether_model::PointType::Signal => return None,
        };
        (target.channel_id, kind, target.point_id)
    };
    reader
        .read_channel(channel_id, kind, channel_point_id)
        .ok()
        .flatten()
        .map(|value| (value.value(), value.timestamp_ms()))
}

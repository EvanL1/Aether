//! Source-query and processor capabilities for Aether Data Processing.

use aether_domain::{
    BindingIdentity, DataProcessingRequest, FeatureDefinition, FeatureRole, HistoryAggregation,
    HistoryDuplicatePolicy, HistoryFeaturePolicy, ProcessingResult, Segment, SegmentKind,
    SourceProvenance, TaskIdentity, TaskKind, TimestampMs,
};
use async_trait::async_trait;

use crate::{PortError, PortErrorKind, PortResult};

fn invalid(message: &'static str) -> PortError {
    PortError::new(PortErrorKind::InvalidData, message)
}

fn feature_names_are_unique(features: &[FeatureDefinition]) -> bool {
    features.iter().enumerate().all(|(index, feature)| {
        !features[..index]
            .iter()
            .any(|seen| seen.name() == feature.name())
    })
}

/// Bounded logical history request, independent of a storage schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryWindow {
    task: TaskIdentity,
    binding: BindingIdentity,
    features: Vec<FeatureDefinition>,
    start: TimestampMs,
    end: TimestampMs,
    max_samples: usize,
    policies: Vec<HistoryFeaturePolicy>,
    cutoff: Option<TimestampMs>,
}

impl HistoryWindow {
    /// Creates a non-empty half-open logical window `[start, end)`.
    pub fn new(
        task: TaskIdentity,
        binding: BindingIdentity,
        features: Vec<FeatureDefinition>,
        start: TimestampMs,
        end: TimestampMs,
        max_samples: usize,
        aggregation: HistoryAggregation,
        duplicate_policy: HistoryDuplicatePolicy,
    ) -> PortResult<Self> {
        if features.is_empty()
            || features
                .iter()
                .any(|feature| feature.role() != FeatureRole::History)
            || !feature_names_are_unique(&features)
            || start >= end
            || max_samples == 0
        {
            return Err(invalid("history window must be bounded and logical"));
        }
        let policies = features
            .iter()
            .map(|feature| {
                HistoryFeaturePolicy::new(feature.name(), aggregation, duplicate_policy)
                    .map_err(|_| invalid("history feature policy is invalid"))
            })
            .collect::<PortResult<Vec<_>>>()?;
        Ok(Self {
            task,
            binding,
            features,
            start,
            end,
            max_samples,
            policies,
            cutoff: None,
        })
    }

    /// Returns the task identity selecting the source plan.
    #[must_use]
    pub const fn task(&self) -> &TaskIdentity {
        &self.task
    }

    /// Replaces default policies with an exact policy for every requested feature.
    pub fn with_feature_policies(
        mut self,
        policies: Vec<HistoryFeaturePolicy>,
    ) -> PortResult<Self> {
        if policies.len() != self.features.len()
            || self.features.iter().any(|feature| {
                policies
                    .iter()
                    .filter(|policy| policy.feature() == feature.name())
                    .count()
                    != 1
            })
        {
            return Err(invalid(
                "history policies must exactly cover requested features",
            ));
        }
        self.policies = policies;
        Ok(self)
    }

    /// Adds the authoritative observation cutoff when the logical output grid
    /// extends one cadence past it to express interval-end labels.
    pub fn with_cutoff(mut self, cutoff: TimestampMs) -> PortResult<Self> {
        if cutoff < self.start || cutoff >= self.end {
            return Err(invalid("history cutoff must be inside the logical window"));
        }
        self.cutoff = Some(cutoff);
        Ok(self)
    }

    /// Returns the commissioned binding.
    #[must_use]
    pub const fn binding(&self) -> &BindingIdentity {
        &self.binding
    }

    /// Returns requested logical features.
    #[must_use]
    pub fn features(&self) -> &[FeatureDefinition] {
        &self.features
    }

    /// Returns the inclusive window start.
    #[must_use]
    pub const fn start(&self) -> TimestampMs {
        self.start
    }

    /// Returns the exclusive window end.
    #[must_use]
    pub const fn end(&self) -> TimestampMs {
        self.end
    }

    /// Returns the hard response sample bound.
    #[must_use]
    pub const fn max_samples(&self) -> usize {
        self.max_samples
    }

    /// Returns exact task-owned policies for requested features.
    #[must_use]
    pub fn policies(&self) -> &[HistoryFeaturePolicy] {
        &self.policies
    }

    /// Returns the policy for one requested semantic feature.
    #[must_use]
    pub fn policy(&self, feature: &str) -> Option<&HistoryFeaturePolicy> {
        self.policies
            .iter()
            .find(|policy| policy.feature() == feature)
    }

    /// Returns the latest source time that may be observed.
    #[must_use]
    pub const fn cutoff(&self) -> TimestampMs {
        match self.cutoff {
            Some(cutoff) => cutoff,
            None => self.end,
        }
    }
}

/// Bounded request for known-future covariates available at one cutoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovariateWindow {
    binding: BindingIdentity,
    features: Vec<FeatureDefinition>,
    as_of: TimestampMs,
    start: TimestampMs,
    end: TimestampMs,
    max_samples: usize,
}

impl CovariateWindow {
    /// Creates a future window whose data issue cut is `as_of`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding: BindingIdentity,
        features: Vec<FeatureDefinition>,
        as_of: TimestampMs,
        start: TimestampMs,
        end: TimestampMs,
        max_samples: usize,
    ) -> PortResult<Self> {
        if features.is_empty()
            || features
                .iter()
                .any(|feature| feature.role() != FeatureRole::FutureCovariate)
            || !feature_names_are_unique(&features)
            || start <= as_of
            || start >= end
            || max_samples == 0
        {
            return Err(invalid("covariate window must be bounded after its cutoff"));
        }
        Ok(Self {
            binding,
            features,
            as_of,
            start,
            end,
            max_samples,
        })
    }

    /// Returns the commissioned binding.
    #[must_use]
    pub const fn binding(&self) -> &BindingIdentity {
        &self.binding
    }

    /// Returns requested logical covariates.
    #[must_use]
    pub fn features(&self) -> &[FeatureDefinition] {
        &self.features
    }

    /// Returns the source issue cutoff.
    #[must_use]
    pub const fn as_of(&self) -> TimestampMs {
        self.as_of
    }

    /// Returns the inclusive valid-time start.
    #[must_use]
    pub const fn start(&self) -> TimestampMs {
        self.start
    }

    /// Returns the exclusive valid-time end.
    #[must_use]
    pub const fn end(&self) -> TimestampMs {
        self.end
    }

    /// Returns the hard response sample bound.
    #[must_use]
    pub const fn max_samples(&self) -> usize {
        self.max_samples
    }
}

/// A source response retaining per-feature watermarks and issue cuts.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedSegment {
    segment: Segment,
    provenance: Vec<SourceProvenance>,
}

impl SourcedSegment {
    /// Creates a segment with exactly one provenance entry per returned series.
    pub fn new(segment: Segment, provenance: Vec<SourceProvenance>) -> PortResult<Self> {
        if provenance.len() != segment.series().len()
            || segment.series().iter().any(|series| {
                let expected_segment = match series.definition().role() {
                    FeatureRole::History => SegmentKind::History,
                    FeatureRole::FutureCovariate => SegmentKind::FutureCovariates,
                    FeatureRole::Static => SegmentKind::StaticFeatures,
                };
                provenance
                    .iter()
                    .filter(|source| {
                        source.segment() == expected_segment
                            && source.feature() == series.definition().name()
                    })
                    .count()
                    != 1
            })
        {
            return Err(invalid(
                "every sourced series requires exactly one role-aware provenance entry",
            ));
        }
        Ok(Self {
            segment,
            provenance,
        })
    }

    /// Returns the aligned source data.
    #[must_use]
    pub const fn segment(&self) -> &Segment {
        &self.segment
    }

    /// Returns per-feature source provenance.
    #[must_use]
    pub fn provenance(&self) -> &[SourceProvenance] {
        &self.provenance
    }

    /// Splits the response into owned data and provenance.
    #[must_use]
    pub fn into_parts(self) -> (Segment, Vec<SourceProvenance>) {
        (self.segment, self.provenance)
    }
}

/// Queries bounded logical history without exposing database abstractions.
#[async_trait]
pub trait HistoryQuery: Send + Sync + 'static {
    /// Reads one bounded logical window with source provenance.
    async fn query(&self, window: HistoryWindow) -> PortResult<SourcedSegment>;
}

/// Resolves task-declared future covariates with issue-time provenance.
#[async_trait]
pub trait CovariateSource: Send + Sync + 'static {
    /// Resolves one bounded future window.
    async fn resolve(&self, window: CovariateWindow) -> PortResult<SourcedSegment>;
}

/// Whether a processor receives data locally or across a remote egress boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataBoundary {
    /// Processor remains on the local edge host or trusted local composition.
    Local,
    /// Processor receives task data through an approved remote route.
    Remote,
}

/// Discoverable limits and typed contracts of one processor adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataProcessorDescriptor {
    id: String,
    version: String,
    supported_tasks: Vec<TaskKind>,
    supported_contracts: Vec<String>,
    data_boundary: DataBoundary,
    max_frame_samples: usize,
    max_request_bytes: usize,
}

impl DataProcessorDescriptor {
    /// Creates a descriptor with finite frame and wire-size limits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        supported_tasks: Vec<TaskKind>,
        supported_contracts: Vec<String>,
        data_boundary: DataBoundary,
        max_frame_samples: usize,
        max_request_bytes: usize,
    ) -> PortResult<Self> {
        let id = id.into();
        let version = version.into();
        if id.trim().is_empty()
            || version.trim().is_empty()
            || supported_tasks.is_empty()
            || supported_contracts.is_empty()
            || supported_contracts
                .iter()
                .any(|contract| contract.trim().is_empty())
            || supported_tasks
                .iter()
                .enumerate()
                .any(|(index, task)| supported_tasks[..index].iter().any(|seen| seen == task))
            || supported_contracts
                .iter()
                .enumerate()
                .any(|(index, contract)| {
                    supported_contracts[..index]
                        .iter()
                        .any(|seen| seen == contract)
                })
            || max_frame_samples == 0
            || max_request_bytes == 0
        {
            return Err(invalid("processor descriptor is invalid"));
        }
        Ok(Self {
            id,
            version,
            supported_tasks,
            supported_contracts,
            data_boundary,
            max_frame_samples,
            max_request_bytes,
        })
    }

    /// Returns the stable processor identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the processor adapter version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns supported typed task kinds.
    #[must_use]
    pub fn supported_tasks(&self) -> &[TaskKind] {
        &self.supported_tasks
    }

    /// Returns supported processor contract identifiers.
    #[must_use]
    pub fn supported_contracts(&self) -> &[String] {
        &self.supported_contracts
    }

    /// Returns whether this processor supports a task kind.
    #[must_use]
    pub fn supports(&self, kind: TaskKind) -> bool {
        self.supported_tasks.contains(&kind)
    }

    /// Returns whether this processor supports an exact contract.
    #[must_use]
    pub fn supports_contract(&self, contract: &str) -> bool {
        self.supported_contracts
            .iter()
            .any(|supported| supported == contract)
    }

    /// Returns the processor's data-egress boundary.
    #[must_use]
    pub const fn data_boundary(&self) -> DataBoundary {
        self.data_boundary
    }

    /// Returns the maximum aggregate scalar-cell count accepted in one frame.
    #[must_use]
    pub const fn max_frame_samples(&self) -> usize {
        self.max_frame_samples
    }

    /// Returns the maximum encoded request size.
    #[must_use]
    pub const fn max_request_bytes(&self) -> usize {
        self.max_request_bytes
    }
}

/// Current processor readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessorHealth {
    /// Ready for normal work.
    Healthy,
    /// Reachable but operating with reduced capability.
    Degraded,
    /// Unable to accept work.
    Unavailable,
}

/// Executes one complete, request-driven processing task.
///
/// The port deliberately exposes no callback or Aether data-source handle. A
/// processor can execute only from the supplied [`DataProcessingRequest`] and
/// its own algorithm artifacts.
#[async_trait]
pub trait DataProcessor: Send + Sync + 'static {
    /// Returns static discovery metadata.
    fn descriptor(&self) -> &DataProcessorDescriptor;

    /// Reports current readiness without processing task data.
    async fn health(&self) -> PortResult<ProcessorHealth>;

    /// Processes one complete frame and returns an untrusted typed result.
    async fn process(&self, request: DataProcessingRequest) -> PortResult<ProcessingResult>;
}

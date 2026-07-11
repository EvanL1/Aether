//! Machine-discoverable application capabilities.

/// Whether a capability reads state or may mutate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    /// Read-only operation.
    Query,
    /// State-changing operation.
    Command,
}

/// Operational risk used by authorization and confirmation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Observation or local computation with no state mutation.
    Low,
    /// Bounded processing/egress or a reversible configuration mutation.
    Medium,
    /// Device control, restart, upgrade, or another high-impact mutation.
    High,
}

/// Human confirmation requirement for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationPolicy {
    /// Confirmation is not required.
    Never,
    /// Deployment policy decides whether confirmation is required.
    Policy,
    /// Explicit confirmation is always required.
    Always,
}

/// Whether the application must durably record an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPolicy {
    /// The read-only operation does not require an audit record.
    NotRequired,
    /// The operation fails closed when its mandatory audit cannot be recorded.
    Required,
}

/// Static metadata shared by CLI, MCP, and optional HTTP transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    name: &'static str,
    kind: OperationKind,
    risk: RiskLevel,
    required_permission: &'static str,
    confirmation: ConfirmationPolicy,
    audit: AuditPolicy,
    idempotent: bool,
}

impl CapabilityDescriptor {
    /// Creates a capability descriptor.
    #[must_use]
    pub const fn new(
        name: &'static str,
        kind: OperationKind,
        risk: RiskLevel,
        required_permission: &'static str,
        confirmation: ConfirmationPolicy,
        audit: AuditPolicy,
        idempotent: bool,
    ) -> Self {
        Self {
            name,
            kind,
            risk,
            required_permission,
            confirmation,
            audit,
            idempotent,
        }
    }

    /// Returns the globally unique capability name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Returns whether the operation is a query or command.
    #[must_use]
    pub const fn kind(self) -> OperationKind {
        self.kind
    }

    /// Returns the risk classification.
    #[must_use]
    pub const fn risk(self) -> RiskLevel {
        self.risk
    }

    /// Returns the permission required to invoke the capability.
    #[must_use]
    pub const fn required_permission(self) -> &'static str {
        self.required_permission
    }

    /// Returns the confirmation rule.
    #[must_use]
    pub const fn confirmation(self) -> ConfirmationPolicy {
        self.confirmation
    }

    /// Returns whether an explicit confirmation is always required.
    #[must_use]
    pub const fn requires_confirmation(self) -> bool {
        matches!(self.confirmation, ConfirmationPolicy::Always)
    }

    /// Returns whether durable auditing is mandatory for the capability.
    #[must_use]
    pub const fn audit_policy(self) -> AuditPolicy {
        self.audit
    }

    /// Returns whether retrying with the same request identity is safe.
    #[must_use]
    pub const fn is_idempotent(self) -> bool {
        self.idempotent
    }
}

/// Read one current point value.
pub const READ_POINT_CAPABILITY: CapabilityDescriptor = CapabilityDescriptor::new(
    "device.read_point",
    OperationKind::Query,
    RiskLevel::Low,
    "device.read",
    ConfirmationPolicy::Never,
    AuditPolicy::NotRequired,
    true,
);

/// Write one command/action point.
pub const WRITE_POINT_CAPABILITY: CapabilityDescriptor = CapabilityDescriptor::new(
    "device.write_point",
    OperationKind::Command,
    RiskLevel::High,
    "device.control",
    ConfirmationPolicy::Always,
    AuditPolicy::Required,
    false,
);

/// Discover configured data-processing tasks and bindings.
pub const TASKS_LIST_CAPABILITY: CapabilityDescriptor = CapabilityDescriptor::new(
    "data_processing.tasks.list",
    OperationKind::Query,
    RiskLevel::Low,
    "data_processing.read",
    ConfirmationPolicy::Never,
    AuditPolicy::NotRequired,
    true,
);

/// Discover current processor readiness without sending task data.
pub const PROCESSOR_HEALTH_CAPABILITY: CapabilityDescriptor = CapabilityDescriptor::new(
    "data_processing.processors.health",
    OperationKind::Query,
    RiskLevel::Low,
    "data_processing.read",
    ConfirmationPolicy::Never,
    AuditPolicy::NotRequired,
    true,
);

/// Assemble a governed frame and request derived data from a processor.
pub const PROCESS_DATA_CAPABILITY: CapabilityDescriptor = CapabilityDescriptor::new(
    "data_processing.process",
    OperationKind::Query,
    RiskLevel::Medium,
    "data_processing.run",
    ConfirmationPolicy::Policy,
    AuditPolicy::Required,
    false,
);

const CAPABILITY_CATALOG: [CapabilityDescriptor; 5] = [
    READ_POINT_CAPABILITY,
    WRITE_POINT_CAPABILITY,
    TASKS_LIST_CAPABILITY,
    PROCESSOR_HEALTH_CAPABILITY,
    PROCESS_DATA_CAPABILITY,
];

/// Returns the transport-neutral capability catalog.
#[must_use]
pub const fn capability_catalog() -> &'static [CapabilityDescriptor] {
    &CAPABILITY_CATALOG
}

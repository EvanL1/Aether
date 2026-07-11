# ADR-0009: Introduce optional, industry-neutral data processing

## Status

Accepted on 2026-07-11. Implementation notes reconciled on 2026-07-11.

## Context

AetherEMS needs to use an existing load and photovoltaic forecasting service.
The important integration boundary is the data path, not the forecasting
algorithm. Aether already owns live and historical observations, point
identity, units, commissioning mappings, data quality, permissions, and the
device-control boundary. A forecasting service should not reconstruct those
concepts by opening Aether databases, reading SHM, or resolving a site ID back
into platform data.

The first production adapters govern freshness, gaps, missingness, ranges,
provenance, and issue time, but they do not yet preserve device-origin sample
quality end to end. The current history schema stores numeric observations
without quality, and the SHM bridge marks accepted finite values as `good`.
That limitation is a commissioning gate where original device quality matters.

Forecasting is also only one example of a broader IoT requirement. Building,
manufacturing, agriculture, transport, and energy distributions may need
windowed aggregation, feature derivation, state estimation, anomaly scoring,
quality classification, remaining-useful-life calculation, or forecasting.
Some implementations use machine-learning inference. Others are deterministic
algorithms, statistical methods, simulations, or remote domain services.

Making model inference the core abstraction would incorrectly imply that all
derived data comes from a model. Making forecasting the abstraction would
embed the first EMS use case into the industry-neutral kernel. Conversely, a
generic script runner or `run(json)` endpoint would erase schemas, data
authority, quality policy, provenance, and safety boundaries.

Aether therefore needs a bounded application-level capability that assembles
governed data, invokes a pluggable processor, validates the response, and
returns explicitly identified derived data. It does not need a second data
plane, a mandatory seventh process, a training platform, or a general ETL/BI
system.

## Decision

### Name and stable vocabulary

The capability is named **Aether Data Processing**. Its stable vocabulary is:

| Concept | Name |
| --- | --- |
| Architecture module | `aether-data-processing` |
| Reserved future orchestration runtime | `aether-data-processor` |
| Application boundary | `DataProcessingApplication` |
| Declarative task | `DataProcessingTask` |
| Pluggable computation port | `DataProcessor` |
| Governed task input | `ProcessingFrame` |
| Processor response | `ProcessingResult` |
| Validated output | `DerivedData` |

The architecture module is a capability boundary, not necessarily a process.
Its domain types, ports, application orchestration, schemas, and conformance
tests follow Aether's existing one-way dependency rules. Version 1 composes it
inside opt-in `aether-api` and isolates the model in a processor sidecar.

Data processing means a bounded execution that consumes a declared snapshot
of governed data and produces a typed derived result. Bounded means that the
task declares its inputs, output schema, temporal scope, quality policy,
deadline, size limits, and permitted publication targets. Arbitrary code
execution and unrestricted data queries are not Data Processing capabilities.

Forecasting is the first `DataProcessingTask`. The existing load-forecasting
service is adapted as its first request-driven `DataProcessor`. Model inference is an
implementation technique inside that processor, not an Aether architectural
concept or requirement.

### Application responsibility

`DataProcessingApplication` owns the complete use case:

1. resolve an enabled, versioned `DataProcessingTask`;
2. authorize the caller and the task's data access and egress policy;
3. read the declared inputs through narrow Aether ports;
4. align and validate those inputs into a complete `ProcessingFrame`;
5. invoke the selected `DataProcessor` with a deadline and size limits;
6. validate the returned `ProcessingResult` against the task contract;
7. stamp accepted output with Aether provenance as `DerivedData`; and
8. return that derived data to the v1 caller. Any future publication path is a
   separate task-approved capability.

The application, not the processor, owns Aether data access and any future
publication.
A processor receives neither SHM layout knowledge nor historian, pack,
configuration, result-store, alarm, rule, or device-control credentials merely
because it performs a computation.

Version 1 requests task execution on demand through the application/HTTP API.
A future composition may trigger or schedule the same application use case;
that scheduling mechanism is not implemented by this decision's v1 landing.

### Typed task contracts

Each `DataProcessingTask` has a stable task kind and versioned input and output
schema. A public caller invokes a typed use case such as `RunForecast`; it does
not choose an arbitrary binary, tensor, SQL query, or unvalidated JSON shape.

A task declaration contains at least:

- a stable task ID, kind, and schema version;
- named input bindings and their source capabilities;
- historical windows, sampling resolution, and alignment policy where used;
- units, sign conventions, value domains, and missing-data policy;
- required watermarks, freshness, completeness, and quality thresholds;
- a logical processor policy rather than a vendor command;
- a typed result schema, validity interval, and maximum result size;
- permitted publication destinations and retention policy;
- data classification and remote-egress policy; and
- execution deadline, retry classification, operation idempotency metadata, and
  fallback policy. The v1 `data_processing.process` operation is explicitly
  non-idempotent.

Industry packs own domain-specific task declarations and point bindings. For
example, the energy pack defines what `site-load`, `pv-power`, import-positive
power, forecast horizon, and weather covariates mean. Core code understands
task and frame invariants, not EMS point names.

The first `Forecast` contract defines a time-indexed future series and may
define quantiles or confidence intervals. Later task kinds may define other
typed payloads. They share the processing envelope but do not collapse into a
single untyped result map.

### ProcessingFrame

`ProcessingFrame` is a complete, immutable snapshot of the data required for
one execution. It contains:

- the observation cutoff (`as_of`) and requested temporal scope;
- semantically named observation series and future covariates;
- timestamps, units, quality flags, gaps, and source watermarks;
- explicitly permitted static context;
- source provenance; and
- aggregate frame quality.

Request, task, binding, contract, deadline, options, and canonical input digest
belong to the outer `DataProcessingRequest`. Keeping correlation outside the
frame makes `ProcessingFrame` a data snapshot rather than a second execution
envelope.

Complete means that a conforming processor can execute the task from the frame
and its own configured algorithm artifacts. It must not call back into Aether
to discover points or fetch required task inputs. Wire encoding may later move
from JSON to a bounded binary format, but the logical frame and its
self-contained semantics remain the same.

"Snapshot" describes the assembled frame, not a guarantee that the current
historian can reconstruct what was known at a past `as_of`. Embedded history
rows have no ingestion/system time or source/configuration epoch. A captured
frame is immutable; a later query for the same event-time window may still see
a late backfill or another physical source hidden behind the same logical
series. Point-in-time replay needs a frozen source snapshot or a bitemporal,
epoch-bearing `HistoryQuery`.

Aether resolves point identity, samples `LiveState`, queries historical
windows, obtains declared external covariates, aligns timestamps, and
verifies that commissioned source units, scale, offset, point kind, and target
sign convention already match the task. Version 1 performs no runtime
unit/sign conversion. A model-backed processor
may still own model-specific feature ordering, scalers, tensor shapes,
framework preprocessing, model execution, and inverse transformation. Those
details do not leak into `ProcessingFrame`.

### ProcessingResult and v1 return

A `DataProcessor` returns a `ProcessingResult`; it does not publish directly.
The response contains the task-specific typed payload plus:

- request, task, and contract identifiers;
- the input digest supplied with the frame;
- processor identity, version, and immutable artifact digest when available;
- execution timestamps and explicit status;
- output units, validity interval, and quality metadata; and
- an identified fallback, if one was used.

The application treats this response as untrusted until validation succeeds.
It rejects mismatched IDs or digests, stale or impossible timestamps,
non-finite values, wrong dimensions, unit mismatch, oversized payloads,
unsupported schemas, and undeclared fallback modes.

After validation, the application creates `DerivedData` with an Aether-owned
result ID, acceptance timestamp, task provenance, input digest, processor
provenance, quality, and expiry. The v1 `data_processing.process` query returns
that value directly to its caller; it has no replay store or result cache.
Updating a
shared derived-data view, writing a durable result repository, or emitting to
  another application is a future, separately configured capability whose
side effects determine its command policy. A processor cannot select or
bypass any publication target.

`produced`, `fallback`, `rejected`, and `unavailable` remain distinguishable
outcomes. A timeout, missing input, or processor failure must never become an
ordinary zero value or an apparently healthy result.

### Data path and processor direction

The data path is push-to-processor:

```text
authenticated HTTP / in-process application trigger
                       |
                       v
            DataProcessingApplication
              |         |          |
              |         |          +-- CovariateSource
              |         +------------- LiveState (read-only)
              +----------------------- HistoryQuery
                       |
             industry-pack task binding
                       |
                       v
              complete ProcessingFrame
                       |
                       v
                  DataProcessor
                       |
                       v
              untrusted ProcessingResult
                       |
                  validate and stamp
                       |
                       v
                   DerivedData
                       |
             return / approved result port
```

The application layer obtains source data through small capability ports. The
default production source extension opens the existing `aether-history.db`
lazily in read-only/query-only mode and reads every feature for one request in
one SQLite transaction. It aggregates raw `(t-c, t]` observations to
interval-end label `t`, with the history grid ending at `as_of`; future values
begin at `as_of+c`. An optional loopback HTTP adapter accepts only an already
aligned `last/reject` grid. A processor adapter may invoke a local Python
sidecar or a remote service, but no transport becomes a domain contract.

One transaction gives invocation-time read consistency only. It is not a
bitemporal cut, and persisted `history_config.storage_*` describes saved
backend intent rather than attesting the active in-memory writer after a
storage `PUT`. Storage transitions require processing to be disabled, the
historian to reconnect/restart and validate a sentinel series, and
`aether-api` to restart against the applied path.

Production also enforces the read-only direction with OS permissions: the API
gets an independently permissioned historian database/WAL/SHM directory,
separate from its writable configuration/audit store. The base shared
`/app/data:rw` mount is not the accepted boundary merely because the adapter
sets SQLite read-only flags.

Artifact version and digest identify selected bytes, but v1 does not carry
`trained_through` or `available_at`. Historical model evaluation must freeze
the artifact registry at its evaluation cut until those chronology fields are
part of commissioning and validation.

LiveState may replace a final interval cell only for a feature aggregated with
`Last`. An instantaneous SHM value is not a valid `Mean`, `Sum`, `Min`, or
`Max` bucket. The energy load/PV targets use `Mean`, so their task declarations
forbid live tail.

Processor selection is made only by a composition root according to the
task's logical policy. A `DataProcessor` may be:

- an in-process deterministic Rust implementation;
- a local native or Python service;
- an accelerator-backed model runtime;
- a remote statistical or simulation service; or
- a deterministic fake used by tests.

Core crates never depend on ONNX Runtime, RKNN, Python, an HTTP framework,
InfluxDB, Redis, PostgreSQL, or a concrete forecasting service. The existing
load-forecasting service's Aether-facing endpoint receives a complete
`ProcessingFrame`. Its legacy reverse-read mode may remain temporarily for
non-Aether clients, but it is not the Aether data path.

### Data authority

Data Processing does not change existing authority:

| Data | Authority |
| --- | --- |
| Current acquired point state | IO-owned SHM through `LiveState` |
| Historical observations | Configured `HistoryQuery` source |
| Task meaning and point bindings | Active industry pack and configuration |
| External covariates | Configured source with timestamp and provenance |
| Algorithm or model artifacts | Selected `DataProcessor` implementation |
| Processing frame | Immutable execution snapshot, not an authority |
| Processor response | Untrusted until application validation |
| Accepted derived result | Configured derived-data result authority |
| Device command | Existing control application and IO dispatch path |

`DerivedData` is authoritative only as the accepted result of its named task,
for its declared validity interval. It does not become authority for the
underlying measured points, source history, alarm state, or device commands.

Derived data must not be written into the IO-owned Telemetry/Signal SHM plane
or disguised as an acquired measurement. If a future low-latency derived-state
plane is required, it must have a separate namespace, writer ownership,
provenance contract, and ADR. A historian may retain derived series only when
its schema explicitly distinguishes them from acquired observations.

### Boundary with neighboring subsystems

Aether Data Processing is deliberately narrower than the ordinary meaning of
"data processing."

#### IO protocol decoding

Protocol adapters decode device bytes, validate wire-level frames, and map
them to normalized point samples inside `aether-io`. That deterministic
acquisition path precedes Data Processing and must continue when no processor
is configured. Protocol decoding is not a `DataProcessingTask`.

#### SHM data plane

SHM transports and owns the current acquired point view. It does not host
arbitrary task execution. Data Processing receives read-only `LiveState` and
cannot write the IO-owned slots.

#### History

History samples, persists, and queries time-indexed observations. Data
Processing reads bounded windows through `HistoryQuery`. Its default extension
opens the existing database read-only without owning schema migration or
retention; processors receive no database access. Data Processing does not turn
the historian into a general compute engine.

#### Rule engine

Rules evaluate declared conditions and deterministic actions. A rule may
consume already published `DerivedData` or trigger an application request, but
remote or long-running processor calls do not execute inside the rule hot
path. A processor cannot dispatch a rule action.

#### Alarm lifecycle

A processor may produce an anomaly score or classified condition as derived
data. `aether-alarm` still owns alarm evaluation, deduplication, occurrence,
acknowledgement, suppression, and recovery. A processing result is not itself
an alarm lifecycle event.

#### ETL, BI, and reporting

Data Processing is not an unbounded warehouse transformation engine, data
lake, dashboard query service, or cross-system bulk replication framework.
Those systems may consume published derived data through extensions. Edge
processing tasks remain bounded, versioned, and commissioned.

#### Planning and control

A task may produce a recommendation, forecast, or candidate plan as derived
data. Optimization policy, plan approval, interlocks, device command routing,
and command audit remain separate application/control responsibilities. No
`DataProcessor` receives command authority, and a result never has an implicit
control side effect.

#### AI control plane

AI is a governed caller and interpreter of Data Processing capabilities. A
processor may internally use AI or ML, but the module is not synonymous with
an LLM agent. Neither AI nor Data Processing enters hard real-time acquisition
or safety loops.

### Runtime composition

The `aether-data-processing` module's lightweight types, ports, application
contracts, schemas, and conformance tests belong to the Aether kernel.
Concrete processors and transports are optional extensions.

ADR-0001 and ADR-0004 continue to define the default six independently
supervised production services. This decision adds neither a mandatory
process nor an external-service dependency to the default distribution.

The implemented `data-processing` profile enables `DataProcessingApplication`
inside `aether-api` and starts the isolated Load-Forecasting processor sidecar.
It does not run a standalone Aether orchestration service, result cache, or
scheduler.

`aether-data-processor` is reserved as a possible future orchestration-runtime
name if shared result publication, scheduling, or remote-processor mediation
later justify another process. A future implementation must compose the same
ports and enforce the same task contracts. It is not the name of the Python
forecasting sidecar.

With no enabled task and processor, processing capabilities are absent or
reported as unavailable. The default six services still install, start, and
behave correctly. No empty runtime starts merely to satisfy a topology
convention.

### AI-native and safety contract

Version 1 exposes authenticated HTTP routes backed by the typed
`DataProcessingApplication`; CLI and MCP bindings remain future work. Every
exposed operation declares query/command classification, risk, required
permission, idempotency, confirmation policy, timeout, and audit behavior.
Future transports must use the same application capability rather than call a
processor directly.

A preview that returns validated data without publication is normally a
query. An invocation that publishes or replaces `DerivedData` is classified
according to that side effect and the task's consumers. Task changes,
processor activation, schedules, egress policy, publication policy, and result
deletion are commands with separate permissions and confirmation rules.

The following invariants apply:

1. Data access is task-scoped. A processor cannot use a declared input as a
   route to query arbitrary Aether points.
2. Remote egress is deny-by-default. Destinations, allowed fields,
   credentials, payload limits, and retention are explicitly configured.
3. Raw observation values are not copied into ordinary audit logs. Audits
   record task, actor, data classification, frame digest, processor,
   destination, result digest, publication targets, and outcome.
4. Processor calls have deadlines, cancellation, payload bounds, and resource
   limits appropriate to their deployment. Failure degrades only the affected
   task.
5. Inputs and outputs are schema-validated and bounded. Stale, malformed,
   non-finite, unit-incompatible, oversized, unverifiable, or expired data
   fails closed.
6. Fallback is explicit and task-approved. It is never hidden as a normal
   output.
7. Processor output is data, not authority to control. Device actions remain
   deny-by-default, confirmed where required, and audited through the existing
   control application.
8. Acquisition, alarms, deterministic safety rules, and the default runtime
   remain correct when every processor is disconnected.

The repository-owned `ai/evals/data-processing.yaml` maps AI-facing permission,
confirmation, audit, non-idempotency, untrusted-result, failure-isolation, and
no-control scenarios to deterministic Rust test evidence. It is a declarative
fixture, not a separate eval runner. Transport-specific CLI/MCP evaluation
remains follow-up work as those interfaces are wired.

## Naming alternatives considered

- `Forecast` and `Prediction` are too narrow for non-future derived data and
  would make the first EMS use case a kernel identity.
- `Inference` and `Model` describe implementation techniques. They exclude
  deterministic, statistical, simulation, and rules-free computations.
- `Analytics` overlaps history exploration, BI, aggregation, reporting, and
  optimization without defining an execution boundary.
- `Transform` suggests stateless shape conversion and overlaps protocol
  decoding and ETL terminology.
- `Compute` and `Processing` alone are too generic to communicate that the
  governed subject is Aether data.
- `AI` confuses numerical processing with Aether's LLM/agent control plane.
- `Data Processing` is selected because it names the governed input/output
  lifecycle while remaining neutral about algorithm and industry. Its broad
  ordinary meaning is constrained by the bounded task contract in this ADR.

## Architectural alternatives considered

### Let each processor fetch Aether data

Rejected. It duplicates point resolution and time alignment, distributes SHM
and historian credentials, couples processors to storage layout, weakens
egress policy, and makes a reproducible governed data cut difficult to prove.

### Let processors publish their own results

Rejected. Aether could not validate output before it becomes visible, enforce
task-approved destinations, stamp consistent provenance, or atomically reject
partial results. Processors return results; the application publishes them.

### Expose a generic script, JSON, or tensor runner

Rejected as the public application contract. It leaks implementation details,
moves schema and authorization decisions into callers, and creates an
arbitrary-code surface. A processor adapter may use a vendor-specific wire
format behind the typed port.

### Put forecasting inside history or automation

Rejected. History owns observation persistence and query. Automation owns
rules and actions. Making either a model host would blur authority and make a
cross-industry application capability depend on one consumer.

### Embed model runtimes in the kernel

Rejected. Model inference is only one implementation. Framework dependencies,
accelerators, Python environments, and model release cadence belong in
optional processor extensions.

### Add a mandatory seventh service

Rejected. Sites without processing tasks should not pay installation,
supervision, memory, or failure-domain costs. Contracts belong to the kernel;
process isolation remains an explicit deployment choice.

## Consequences

### Positive

- Forecasting becomes the first use case of a reusable IoT data-processing
  capability rather than a new energy or model dependency in the kernel.
- Aether retains ownership of source semantics, data quality, authorization,
  validation, provenance, and publication.
- Deterministic code, local model runtimes, accelerators, and remote services
  remain replaceable processor implementations.
- Captured, content-identifiable frames can be replayed in conformance tests
  and frozen-input evaluations without granting processors platform-storage
  access.
- Derived data remains visibly distinct from acquired measurements, alarm
  state, and device commands.
- AI clients can discover and explain processing tasks without gaining a
  shortcut around data-egress or control safety.
- Deployments without processing retain the six-service,
  no-external-service default.

### Negative

- Aether must define and version task, frame, result, derived-data, quality,
  publication, and provenance schemas.
- Data assembly, time alignment, and validation become explicit application
  responsibilities with substantial conformance-test requirements.
- Complete frames can be larger than model-specific tensors; adapters must
  enforce bounds and may later need a compact encoding.
- Publication introduces lifecycle questions for retention, replacement,
  expiry, consumer notification, and comparison with later actuals.
- Existing services that fetch their own data require a compatibility period
  and a new frame-oriented endpoint.
- The current historian cannot by itself provide point-in-time historical
  cuts: rows lack ingestion time and source/binding epoch, and saved storage
  configuration does not attest an unreconnected active writer.
- Direct SQLite composition needs a separate read-only historian filesystem
  boundary for the API; the current base Compose data mount does not provide
  it.
- Artifact provenance lacks training and availability cuts, so a digest alone
  does not prevent model-vintage leakage in historical evaluation.
- Enabling the opt-in API/sidecar composition introduces authentication,
  health, and resource-isolation work. Scheduling remains a separate future
  capability.

## Migration plan and removal criteria

1. Add versioned, industry-neutral types and schemas for
   `DataProcessingTask`, `ProcessingFrame`, `ProcessingResult`, `DerivedData`,
   and the first typed `Forecast` payload.
2. Add narrow query ports for historical windows and covariates where existing
   ports do not express them. Preserve read-only `LiveState`; do not broaden
   `HistorySink` into a vendor-shaped database abstraction.
3. Implement `DataProcessingApplication` with deterministic assembly, input
   policy, processor deadlines, output validation, provenance stamping, and
   typed response errors before adding a production processor.
4. Add processor and result-validation conformance suites plus a deterministic fake
   processor. Cover ordering, gaps, duplicate timestamps, stale watermarks,
   time-zone boundaries, non-finite values, wrong result shapes, timeout,
   fallback, and unavailable outcomes.
5. Adapt the existing load-forecasting service as the first AetherEMS
   `Forecast` processor. Its Aether endpoint accepts a complete
   `ProcessingFrame`. Run it in shadow mode with no control authority.
6. Add load and photovoltaic processing-task declarations to the energy pack,
   including bindings, units, sign conventions, windows, resolution, horizon,
   quality limits, egress policy, processor policy, and result destinations.
7. Expose task discovery, processor health, and invocation through the common
   capability catalog and authenticated v1 HTTP transport. Add CLI/MCP,
   processor activation, publication, and schedule capabilities only after
   their permission, confirmation, rollback, and audit contracts exist.
8. Keep `aether-data-processor` reserved; add it only if future process-level
   orchestration requirements justify it. Its installation and failure must
   remain isolated from the default six services.

The forecasting service's reverse-read/InfluxDB endpoint may remain as an
explicitly documented compatibility mode for non-Aether callers. It may be
removed from the AetherEMS deployment only after all active site mappings use
complete-frame requests, shadow comparisons meet declared acceptance
thresholds, pinned-frame golden comparisons are available, rollback is tested,
and no
Aether composition depends on processor-side data access.

## Acceptance and verification criteria

This decision was accepted after the task and frame contracts, result
semantics, data-authority boundary, remote-egress policy, and opt-in
`aether-api`/processor-sidecar composition were reviewed together and landed as
repository-owned domain, port, application, codec, schema, adapter, testkit,
and AetherEMS pack assets.
Acceptance records the architectural decision; production commissioning and
site-specific shadow validation remain deployment gates.

The first implementation is conformant only when all of the following are
true:

1. Core processing contracts and the default six services compile without a
   model framework, Python, InfluxDB, Redis, PostgreSQL, or concrete processor.
2. The minimal gateway passes its composition contract with no processing task
   or processor route configured.
3. Processor conformance tests prove that a complete frame is sufficient and
   that the processor needs no SHM, historian, pack, or Aether configuration
   access.
4. Authority tests prove that Data Processing cannot write IO-owned live state,
   create alarm lifecycle state, execute rule actions, or dispatch control.
5. Contract tests reject stale, malformed, non-finite, unit-incompatible,
   oversized, dimensionally invalid, and unverifiable processor responses.
6. Response tests prove validate-before-return behavior, provenance retention,
   expiry, and explicit derived-data identity. Durable publication remains a
   separate future capability.
7. AetherEMS fixtures and composition tests assemble the commissioned load
   frame and retain input and algorithm provenance. The PV task remains disabled
   until its full mapping and processor path pass equivalent tests.
8. Capability-policy tests cover the landed descriptors, and the declarative
   AI eval fixture links authorization, egress, audit, failure, and
   unauthorized-control scenarios to existing Rust evidence. There is no
   independent eval runner.
9. Enabling or disabling the opt-in Data Processing profile does not alter
   correctness or startup of acquisition, history, alarms, uplink, API, or
   deterministic automation.
10. Architecture checks prevent core-to-concrete-processor dependencies and
    prevent a processor extension from entering the default runtime graph.

The repository verification sequence remains:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --bins
./scripts/check-architecture.sh
```

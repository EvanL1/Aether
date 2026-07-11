# Aether Data Processing Implementation Plan

**Status**: Implemented v1 baseline; production site commissioning and shadow cutover remain blocked on the gates below
**Recorded**: 2026-07-11
**Reconciled**: 2026-07-11

## Goal

Add an industry-neutral, request-driven Data Processing capability to Aether,
then prove the boundary in AetherEMS by adapting the existing load/PV
forecasting service as the first `DataProcessor`.

The implementation is complete only when all of the following are true:

- Aether owns data discovery, bounded window assembly, quality policy, and
  authorization. The current sources do not yet preserve device-origin sample
  quality end to end.
- A processor receives a complete `ProcessingFrame` and cannot read Aether
  SHM, configuration, or history storage directly.
- Processing results are typed, validated, traceable derived data and cannot
  silently become live-state authority or a device command.
- The default six-service runtime builds and runs without a processor or any
  Python/model dependency.
- AetherEMS declares disabled-by-default load and PV forecast tasks and can run
  one end-to-end against the request-driven Load-Forecasting processor.

## Stable vocabulary

| Concept | Name |
|---|---|
| Architecture capability | Aether Data Processing |
| Reserved future orchestration runtime | `aether-data-processor` |
| Application boundary | `DataProcessingApplication` |
| Declarative unit of work | `DataProcessingTask` |
| Execution port | `DataProcessor` |
| Bounded input | `ProcessingFrame` |
| Untrusted processor response | `ProcessingResult` |
| Validated, non-authoritative result | `DerivedData` |

Forecasting is the first task kind. Model inference is an implementation
detail of one processor, not a core Aether concept.

## Target data path

```text
authenticated HTTP / in-process application call
            │ typed task invocation
            ▼
DataProcessingApplication
  ├─ task bindings from an industry pack
  ├─ HistoryQuery
  ├─ LiveState (read-only, Last-only current tail)
  └─ request/context inputs
            │
            ▼ complete ProcessingFrame
       DataProcessor
            │
            ▼ untrusted ProcessingResult
       validate and stamp DerivedData
       authenticated HTTP response
```

There is no processor-to-Aether callback for data discovery. The model or
algorithm endpoint does not receive `plant_id` and then query an internal
database; it receives the values required for the selected task.

## Phase 1: Documentation and contracts

1. Add ADR-0009 and the concept, flow, processor, contract, and EMS forecasting
   pages.
2. Index every page in `llms.txt`, `docs/README.md`, architecture entry points,
   and AI runbooks/invariants.
3. Define application-facing and processor-facing v1 envelopes separately.
   Ordinary callers select a commissioned task; only the internal processor
   request contains a complete frame and resolved processor/model metadata.
4. Record that processing is application-level bounded computation. It does
   not replace protocol decoding, the SHM data plane, history, rule execution,
   alarms, bulk ETL/BI, planning, or control.

## Phase 2: Core domain and ports (TDD)

1. Add behavior tests for task/request identifiers, bounded frames, finite
   values, monotonically ordered samples, result status, expiry, and
   provenance.
2. Add an object-safe `HistoryQuery` port that returns bounded logical series
   without exposing a database or HTTP vocabulary.
3. Add an object-safe `DataProcessor` port that accepts the typed processor
   request and returns a typed result with recovery semantics.
4. Extend `aether-testkit` with conformance checks for processor request
   identity, input digest preservation, finite output, and unavailable/error
   behavior.

The domain layer remains `no_std` where practical. Dynamic task/frame values
may use `alloc`; concrete serialization remains in interfaces/extensions.

## Phase 3: Application orchestration (TDD)

1. Add `DataProcessingApplication` as a transport-neutral query use case.
2. Authorize `data_processing.process` through the shared capability policy.
3. Resolve task bindings and request a bounded history window.
4. For `Last`-aggregated features only, read the current tail through
   `LiveState`, replace only mapped final interval cells, and never coerce
   missing/non-finite samples to zero. Reject live tail for aggregate buckets
   such as `Mean` or `Sum`. The
   current SHM bridge synthesizes `good` for accepted values rather than
   preserving device-origin quality.
5. Enforce configured series/sample/payload limits before calling a processor.
6. Calculate a deterministic input digest and invoke `DataProcessor` with a
   deadline.
7. Validate request/task identity, digest, finite/ordered output, quality,
   status, provenance, and expiry before returning a result.

## Phase 4: Adapters and opt-in composition

1. Add a local in-memory history query and recording processor for examples and
   deterministic tests.
2. Add the production read-only SQLite history adapter, with task-scoped
   interval aggregation and one snapshot per logical request. Keep the HTTP
   history adapter as a pre-aligned `last/reject` alternative only.
3. Add `extensions/http-data-processor` using a bounded request body, explicit
   connect/request timeouts, TLS support, typed errors, and no ambient
   credentials.
4. Keep endpoint selection in the composition root. Reject invalid schemes,
   URL credentials, fragments, and uncommissioned task-to-endpoint mappings.
5. Reserve `aether-data-processor`; add that service only if a future
   orchestration boundary is needed beyond the implemented `aether-api` plus
   processor-sidecar composition. It must not join the default six-service
   startup set.

## Phase 5: AetherEMS forecasting landing

1. Add disabled-by-default load/PV task declarations under
   `packs/energy/data-processing/`.
2. Bind semantic feature names to instance measurement points and request/NWP
   inputs; never bind a model to physical channel slots.
3. Adapt the existing Load-Forecasting Edge Platform with a v1 endpoint that
   accepts a complete `ProcessingFrame`. Preserve the legacy identifier-only
   endpoint temporarily, but AetherEMS uses only `/v1/process`.
4. Keep scaler, feature ordering, tensor construction, ONNX/RKNN execution, and
   inverse scaling inside the forecast processor.
5. Return explicit `produced`, `fallback`, or `unavailable` status. A technical
   failure must never return an ordinary all-zero forecast with HTTP 200.
6. Extend `examples/energy-gateway` with a fully local composition test using a
   deterministic forecast processor and the same task contract used by the
   HTTP adapter.
7. Add an opt-in Compose/systemd profile for the Python forecast processor;
   the safe bundled pack remains uncommissioned.

## Phase 6: AI-native surface

Declare, implement, and test these capabilities:

| Capability | Kind | Risk | Permission | Idempotent | Confirmation | Audit |
|---|---|---|---|---|---|---|
| `data_processing.tasks.list` | query | low | `data_processing.read` | yes | never | not required |
| `data_processing.processors.health` | query | low | `data_processing.read` | yes | never | not required |
| `data_processing.process` | query | medium | `data_processing.run` | no | policy | required |

Changing task bindings, processor endpoints, or an approved artifact is
configuration mutation and remains outside the first read-only surface. It
uses the existing governed configuration path rather than adding a
model-specific core capability. Publishing a result into a shared view or an
automatic plan is also a separate command.

Version 1 exposes these capabilities only through the authenticated
`/api/v1/data-processing/*` HTTP routes and the transport-neutral application
API. CLI and MCP bindings remain follow-up work. The process operation has no
exact replay, request de-duplication, built-in cache, or `409` reuse guarantee;
`input_digest` is content identity for correlation and audit.

## Verification gates

Run the narrowest test after each phase, then require:

```bash
cargo test -p aether-domain
cargo test -p aether-ports
cargo test -p aether-application
cargo test -p aether-store-local
cargo test -p aether-sqlite-history-query
cargo test -p aether-http-history-query
cargo test -p aether-http-data-processor
cargo test -p aether-api --bin aether-api
cargo test -p aether-example-energy-gateway --test composition_contract
cargo test -p aether-example-energy-gateway --test data_processing_composition
uvx --from check-jsonschema check-jsonschema --check-metaschema contracts/data-processing/*.schema.json
(cd integrations/load-forecasting && uv run pytest)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --bins
./scripts/check-architecture.sh
```

The forecast processor additionally requires Python unit and API contract tests
through `uv run pytest`, plus an end-to-end test that sends a complete frame
and verifies timestamps, model provenance, status, and finite output.

Production cutover additionally requires a fixed, pinned upstream
Load-Forecasting commit. The inspected commit indexes future load covariates as
`forecast_sorted[step+1]`, skipping the first `as_of+cadence` row. It must be
corrected and covered by a golden step-to-row alignment test. The legacy
runtime's raw feature/scaler/prediction/path prints must be removed; actual
artifact paths must be resolved and pinned through the same model manager used
for execution; redistribution requires an upstream license or explicit
permission; and a real artifact must meet the route deadline at target-hardware
p95. Site fixtures must also prove raw interval semantics—especially that
`rain` is cadence accumulation rather than a rolling total or rate—because v1
does not infer or convert those meanings.

Historical evaluation has three additional gates. First, current history rows
lack ingestion time and source/binding/configuration epoch, so old `as_of`
queries can include later backfills or splice a remapped physical source; use a
frozen historian export or add bitemporal/epoch filtering. Second, artifact
identity has version and digest but no `trained_through` or `available_at`, so
freeze the artifact registry at the evaluation cut. Third,
`history_config.storage_*` is saved intent and a storage `PUT` does not
reconnect the active writer; storage changes require processing disabled,
historian reconnect/restart plus sentinel verification, and an `aether-api`
restart on the applied path. Finally, SQLite read-only connection flags are
not an OS boundary: production must split the historian database/WAL/SHM into
an independently permissioned read-only API mount, rather than the current
base Compose-wide `/app/data:rw` mount.

## Acceptance record

ADR-0009 is Accepted. The repository baseline satisfies the architectural
promotion through the following implementation evidence:

1. Core contracts and application behavior tests pass.
2. At least one local processor and the HTTP processor pass conformance tests.
3. AetherEMS runs the forecast composition without Redis, PostgreSQL, InfluxDB,
   a broker, network access, a field device, or a browser.
4. Architecture checks prove no core crate depends on the HTTP or Python
   implementation.
5. Failure tests prove processor loss cannot stop acquisition, deterministic
   rules, alarms, or device-safety behavior.

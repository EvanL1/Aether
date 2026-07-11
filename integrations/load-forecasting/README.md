# Aether Load-Forecasting Processor

This opt-in Python package adds the request-driven Aether Data Processing v1
boundary to the existing
[`panskai/Load-Forecasting`](https://github.com/panskai/Load-Forecasting)
Edge-Platform. It accepts a complete `ProcessingFrame` at `POST /v1/process`;
it never queries InfluxDB, SHM, Aether history, site configuration, or a reverse
callback.

The adapter keeps feature ordering, scaler use, tensor construction,
ONNX/RKNN selection, model execution, and inverse scaling inside the existing
`InferenceService`. Aether remains responsible for point binding, data
selection, exact unit/sign contract checks, alignment, quality policy,
provenance, and the input digest. The v1 runtime does not convert engineering
units or sign conventions.

`/health` and `/v1/health` remain liveness/readiness endpoints without bearer
authentication. They report `status: unavailable` when no artifact bundle and
path-resolution proof are commissioned, when bundle files changed, or when a
required legacy readiness probe fails. They never invoke inference.

## Production cutover blockers

This adapter is tested, but the pinned upstream Edge-Platform is not ready for
production Aether routing until every item below is closed:

1. Fix the load predictor's `forecast_sorted[step+1]` lookup, which skips the
   first future-covariate row. Pin the fixed upstream commit and add a golden
   test proving step zero consumes `as_of+cadence` and step `n` consumes
   `as_of+(n+1)cadence`.
2. Remove legacy stdout/stderr prints containing model paths, scaler values,
   normalized/real predictions, and next-step features. HTTP redaction cannot
   sanitize process-global output.
3. Supply `artifact_file_resolver(context)` from the same `ModelManager`
   selection used by `run_inference`, and return every actual model, scaler,
   configuration, and auxiliary file. Keep those files pinned and read-only for
   the call.
4. Obtain an upstream license or explicit redistribution/deployment permission.
   This adapter's MIT/Apache licensing does not license the separate upstream
   repository.
5. Benchmark a real commissioned artifact on the target edge hardware. Measured
   p95 must fit within the configured Aether frame-and-processor work deadline
   at the commissioned concurrency; the pinned legacy model-loading and serial autoregressive path
   currently has no qualifying p95 evidence.
6. For historical validation, use a frozen historian export with a frozen
   physical-source epoch. Current history rows have no ingestion time or
   source/binding/configuration epoch, so a later backfill or remap behind the
   same logical series makes a live query unsuitable as a point-in-time cut.
7. Freeze the approved artifact registry at the evaluation cut. Version and
   digest identify model bytes, but v1 carries no `trained_through` or
   `available_at`; an old frame can otherwise use a model created later.
8. Across historian storage changes, keep processing disabled until
   `aether-history` reconnects or restarts, its active SQLite backend plus a
   commissioned sentinel series are verified, and `aether-api` restarts on the
   same path. Persisted `history_config.storage_*` is saved intent and a
   storage `PUT` alone does not switch the active writer.
9. Give `aether-api` an independently permissioned read-only mount/identity for
   the historian database/WAL/SHM directory, separate from its writable
   configuration and audit database. The base Compose `/app/data:rw` mount and
   SQLite read-only flags do not satisfy this production boundary.

The route and readiness probe must remain unavailable until these gates pass.
The Compose deployment combines host-loopback publication with a dedicated
`internal: true` network, mechanically blocking container external egress.
Native/systemd deployment still needs host firewall isolation. Neither example
chooses CPU, memory, or PID quotas; production commissioning must add measured
resource limits so model overload cannot affect deterministic Aether services.

## Contract behavior

- The request body is strictly typed, rejects unknown fields, and is bounded
  by `max_request_bytes` (1 MiB by default).
- Request timestamps use UTC `Z` and no more than millisecond precision; the
  processor rejects higher-precision or offset timestamps instead of silently
  changing the signed digest basis.
- RFC 8785 canonical JSON plus SHA-256 verifies that task, binding, artifact,
  frame, and options match `input_digest`.
- Load history must contain exactly `load`, `temp_avg`, `humidity`, `rain`, and
  `quarter_hour`; the future segment contains the latter four.
- The production policy matches the energy pack: 672 history points at a
  15-minute cadence and at most 288 forecast points. Tests may override the
  history bound only for synthetic compact fixtures.
- Provenance is one-to-one with every frame feature. Calendar values carry
  `calendar` provenance, must equal the UTC quarter-hour encoded by their
  timestamps, and cannot advance the actual-observation watermark.
- Humidity, precipitation, and quarter-hour values are checked against the
  commissioned task ranges before any model code runs.
- Every non-calendar future covariate requires NWP provenance with `issued_at`;
  the processor rejects a missing issue time or one later than the frame's
  `as_of`. This closes the NWP-vintage boundary for the supplied frame; it does
  not cure history-ingestion or model-vintage leakage in an offline backtest.
- The adapter passes only in-memory rows from the request to the legacy
  `InferenceService.run_inference` method. It never invokes its preprocessing
  or InfluxDB path.
- Normal output is `produced`. An approved persistence fallback is explicitly
  labeled `fallback` and uses the last real `load` value in the request.
  Otherwise engine failure is `unavailable`; it is never a healthy all-zero
  response.
- Every timestamp in a `ProcessingResult` is emitted as UTC with exactly three
  fractional digits (for example, `2026-07-11T12:00:02.123Z`), matching the
  Rust codec's millisecond precision.
- HTTP error envelopes are typed and redact model paths, stack traces,
  endpoints, and source data; the separate legacy stdout cutover gate is
  documented below.
- Synchronous model execution uses a dedicated bounded executor. The default is
  one occupied model slot; excess work receives typed HTTP `429`
  (`PROCESSOR_BUSY`). If a caller times out or disconnects, its model thread
  retains the slot until the thread actually finishes.
- `POST /v1/process` can require a deployment-owned bearer token. Credential
  checks use constant-time comparison and run before the body is read. Health
  remains unauthenticated because the example service is loopback-only.
- `/v1/process` uses the versioned media type
  `application/vnd.aether.data-processing+json;version=1`, matching the Rust
  HTTP adapter.

All `POST /v1/process` responses use
`application/vnd.aether.data-processing+json;version=1`, including non-2xx
typed `ProcessingError` envelopes. This keeps one versioned contract family
at the processor boundary. The `/health` and `/v1/health` operational endpoints
continue to use `application/json`.

The disabled PV task is fully declared in the energy pack, but this first
compatibility endpoint intentionally commissions only the load task. PV is
enabled only after its complete 19/20-feature model mapping passes the same
processor tests.

## Add it to the existing Edge-Platform

Copy this package into the Edge-Platform environment (or add this directory as
an editable deployment dependency), then install its locked dependencies with
`uv`. Keep the legacy `/predict` and `/forecast` routes for non-Aether clients;
AetherEMS uses only `/v1/process`.

Add the Aether routes to the existing `app.py` composition root:

```python
from aether_load_forecasting_processor import (
    EdgePlatformInferenceServiceEngine,
    LoadForecastProcessor,
    ProcessorPolicy,
    install_routes,
    load_commissioned_artifact_bundles_from_env,
)
from inference.inference_service import InferenceService

engine = EdgePlatformInferenceServiceEngine(
    InferenceService(),
    artifact_bundles=load_commissioned_artifact_bundles_from_env(),
    artifact_file_resolver=resolve_and_pin_edge_platform_artifact_files,
    readiness_probe=edge_platform_model_is_ready,
)
processor = LoadForecastProcessor(
    engine,
    ProcessorPolicy(allow_persistence_fallback=False),
)
install_routes(app, processor=processor, max_request_bytes=4_194_304)
```

`install_routes` reads the authentication and concurrency environment settings
described below unless explicit `auth_policy=` or
`max_processor_concurrency=` arguments are supplied.

`resolve_and_pin_edge_platform_artifact_files(context)` is a required
deployment bridge, not a helper supplied by this package. It must use the same
`ModelManager` resolution as `run_inference`, pin that selection until the call
returns, and return the complete logical-name-to-absolute-path mapping. The
adapter compares those resolved paths with the commissioned bundle before it
invokes the model. The unmodified legacy service resolves the lexicographically
latest model directory independently and does not expose this proof; therefore
using `InferenceService()` without this resolver is deliberately unavailable
and is a cutover blocker, not a supported production shortcut. Keep the model
tree read-only to prevent a resolution race. `readiness_probe` is also required:
it must return true only after the legacy model is ready and its production
logging policy has been applied. Missing or failing proof keeps health and
processing unavailable.

Model selection remains processor-owned. The Aether artifact selector names an
already approved family/version; a processing request never downloads or
activates a model. Secrets and processor endpoints belong to deployment
configuration, not the energy pack or request body.

The legacy engine must return `body.model_version`, and it must equal the
version requested by Aether. A version label alone is not artifact provenance.
At composition time, every selectable version therefore needs a commissioned
bundle containing the exact model, scaler, feature configuration, and any
other file that the legacy service loads. The adapter hashes the logical file
names, sizes, and bytes with the `aether.artifact.bundle.v1` domain, compares
that result with the expected digest, and fails startup on a mismatch. It also
checks file identity, size, and modification time before each call. The locally
computed digest is returned to Aether; a digest reported by the legacy response
is only an additional consistency check.

If the Edge-Platform composition root cannot identify every file used by a
model version, that version must not be commissioned. The model directory must
be mounted read-only after startup.

The HTTP adapter redacts error envelopes, but it cannot sanitize output that
the legacy implementation writes directly to process stdout/stderr. The
unmodified `InferenceService`/`LoadPredictor` prints model paths, scaler data,
normalized and real predictions, and next-step feature vectors. Production
cutover therefore also requires an upstream logging patch or configuration
that removes those prints; process-global stdout redirection is not considered
a safe substitute. The required readiness probe must stay false until that
logging gate is satisfied.

## Production environment

The standalone and mounted forms understand these settings:

- `AETHER_LOAD_FORECASTING_MAX_CONCURRENCY`: integer in `[1, 256]`; defaults to
  `1`. This bounds both executor workers and occupied model slots.
- `AETHER_LOAD_FORECASTING_BEARER_TOKEN`: enables bearer authentication for
  `POST /v1/process` when present.
- `AETHER_LOAD_FORECASTING_REQUIRE_AUTH`: exactly `true` or `false`. Set it to
  `true` in production so startup fails when the token is missing.
- `AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES`: a non-empty JSON array of strict
  bundle declarations. `load_commissioned_artifact_bundles_from_env()` requires
  it by default.

Example bundle declaration (paths must be absolute regular files and may not
be symlinks):

```json
[
  {
    "kind": "model",
    "family": "site-load",
    "version": "v3",
    "expected_digest": "sha256:<64 lowercase hex characters>",
    "files": {
      "model": "/opt/load-forecasting/artifacts/v3/model.onnx",
      "scaler": "/opt/load-forecasting/artifacts/v3/scaler.json",
      "config": "/opt/load-forecasting/artifacts/v3/features.json"
    }
  }
]
```

Generate the candidate bundle digest during commissioning, before setting its
expected value:

```python
from aether_load_forecasting_processor import compute_artifact_bundle_digest

print(
    compute_artifact_bundle_digest(
        {
            "model": "/opt/load-forecasting/artifacts/v3/model.onnx",
            "scaler": "/opt/load-forecasting/artifacts/v3/scaler.json",
            "config": "/opt/load-forecasting/artifacts/v3/features.json",
        }
    )
)
```

## Run and verify

```bash
cd integrations/load-forecasting
uv sync --all-groups
uv run ruff check .
uv run pytest
```

The package supports Python 3.10 and newer. CI or commissioning should run the
same locked suite with the oldest deployed interpreter, for example
`uv run --python 3.10 pytest`.

For a standalone sidecar, construct `create_app(processor=...)` and run that
FastAPI application with the deployment's process supervisor. Health is
available at both `/health` and `/v1/health`. The default Aether runtime does
not start this process and has no Python or model dependency.

Opt-in Compose and hardened systemd examples are documented in
[`deploy/README.md`](deploy/README.md). Neither deployment receives an Aether
data, configuration, or SHM mount; the processor obtains all task input from
the request body.

The normative contracts and EMS fixtures are in
[`contracts/data-processing`](../../contracts/data-processing/README.md) and
[`packs/energy/data-processing`](../../packs/energy/data-processing/README.md).

## Licensing boundary

This adapter package is available under `MIT OR Apache-2.0`; both license texts
ship with its source and wheel. That does not grant rights to the separate
`panskai/Load-Forecasting` codebase. The upstream repository used for this
integration did not provide an explicit license when reviewed, so publishing
or redistributing a combined container image remains blocked until its owner
provides suitable permission. Recheck upstream licensing at commissioning
time; running this adapter does not resolve that legal boundary.

# Aether Architecture

Aether is migrating from a Redis-centred multi-service EMS product to an
AI-native, industry-neutral edge kernel. The target architecture and migration
rules are defined in:

- [ADR-0001: AI-native edge kernel](docs/adr/0001-ai-native-edge-kernel.md)
- [ADR-0003: Multi-process SHM and event plane](docs/adr/0003-multi-process-shm-event-plane.md)
- [ADR-0004: Canonical service names](docs/adr/0004-canonical-service-names.md)
- [ADR-0009: Aether Data Processing](docs/adr/0009-aether-data-processing.md)
- [Target repository layout](docs/architecture/target-layout.md)
- [AI invariants](ai/invariants.md)
- [Capability safety policy](ai/safety-policy.yaml)

## Current migration state

The default Cargo graph is already external-service-free. It contains the
domain, ports, application layer, SDK, local adapters, the physical SHM data
plane, and the read-only SHM bridge. In particular:

- `aether-dataplane` owns mmap layout, seqlock slots, dirty tracking, and
  snapshots without depending on Redis, SQLx, or the legacy service model.
- `FileOutbox` provides bounded local store-and-forward with crash recovery.
- Redis and PostgreSQL implementations are optional integrations rather than
  prerequisites of the peripheral service data paths.
- `aether-alarm`, `aether-api`, `aether-history`, and `aether-uplink` discover logical points from
  SQLite and read current values directly from SHM. `aether-alarm` and
  `aether-api` also own isolated PointWatch bitmaps and UDS listeners.
- `aether-history` uses embedded SQLite history by default; PostgreSQL/TimescaleDB are
  enabled with the `postgres-storage` feature. `aether-uplink` retains its durable
  local outbox before MQTT.

## Target runtime

The production target is a supervised set of isolated processes: `aether-io`,
`aether-automation`, `aether-alarm`, `aether-history`, `aether-api`, and `aether-uplink`. A crash, blocked
driver, or cloud outage in one process must not take down acquisition or the
other services. They share only explicit local capabilities: SHM for current
state, per-consumer UDS/bitmap event channels, SQLite configuration, and local
HTTP command APIs.

An optional single-process composition may exist for tests, simulation, or
small development profiles. It is not the deployment default and does not
replace the service binaries. Neither profile requires PostgreSQL; Redis is a
compatibility mirror while remaining legacy aether-io/aether-automation paths are migrated.

Optional adapters may add Redis state mirroring, PostgreSQL history, MQTT
uplink, or HTTP APIs. They do not change the source-of-truth rules.

## Data-processing capability

[Aether Data Processing](docs/concepts/data-processing.md) is the implemented
industry-neutral boundary for assembling governed IoT data and invoking a
local or remote processor. The application owns history and live-state
selection, semantic bindings, quality policy, authorization, and result
validation. A processor receives a complete bounded processing frame; it never
reaches back into SHM, the history database, or an industry pack.

The opt-in v1 composition lives in `aether-api`, reads raw history through a
read-only SQLite adapter, and returns `DerivedData` through authenticated
`/api/v1/data-processing/*` routes. The Load-Forecasting implementation remains
an isolated sidecar. CLI/MCP bindings, result caching, scheduling, and a
standalone Aether orchestration process are not implemented. Current history
and SHM adapters also do not preserve device-origin sample quality end to end;
they enforce freshness, gaps, missingness, constraints, and provenance. The
SQLite history is invocation-time consistent but not bitemporal: rows have no
ingestion/source epoch, and model provenance has no training/availability cut.
Therefore historical `as_of` alone is not a leakage-safe backtest boundary.
Production direct-history composition also requires a dedicated read-only
historian directory/identity for the API; SQLite read-only mode over the base
shared writable data mount is not an authority boundary.

Processing results are expiring derived-data artifacts, not live point state
and not device commands. They are never written into the IO-owned T/S segment,
and a forecast, aggregate, estimate, or classification can influence equipment
only through a separate automation/control use case. The domain contracts live
in `aether-domain`, the ports and orchestration live in `aether-ports` and
`aether-application`, and the strict v1 wire codec lives in
`aether-data-processing`. Concrete processors and HTTP transport remain
optional; `aether-data-processor` is only a reserved future process name. The
default six-service runtime continues to operate when no processor is installed.

## Dependency Rule

```text
interfaces ----> application ----> ports ----> domain
                       ^              ^
                       |              |
runtime/composition ---+          extensions
                       |
                  data plane
```

Only a composition root may depend on both application code and concrete
extensions. CI checks the core manifests for forbidden infrastructure
dependencies.

The concrete extraction and local-outbox decisions are recorded in
[ADR-0002](docs/adr/0002-dataplane-and-local-outbox.md).

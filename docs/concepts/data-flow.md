---
title: Data Flow
description: SHM-native uplink and downlink paths end to end, with latency budgets
updated: 2026-07-11
---

# Data Flow

Aether moves data along two independent paths. The **uplink** carries
measurement points — telemetry (T) and signal (S) values — from devices through
aether-io into shared memory, and from there to every consumer. The **downlink**
carries action points — control (C) and adjustment (A) commands — from the rule
engine or the HTTP API through aether-automation back to a device. Live point values and
command transport use the shared-memory segment as the source of truth and
transport. No default service needs Redis or PostgreSQL for live data.

## Uplink (device → consumers)

1. A protocol frame arrives on a communication channel and the channel's
   protocol adapter in aether-io decodes it into point values.
2. aether-io writes each value into its T or S slot in shared memory via
   `UnifiedWriter::set_direct` (`libs/aether-rtdb-shm/src/unified_shm.rs`) —
   ~10 ns per point per the README.
3. **Event path (immediate).** After every slot write, the
   `PointWatchSignaler` (`libs/aether-rtdb-shm/src/point_watch.rs`) checks the
   independent bitmap owned by each event consumer. On a hit, a bounded queue
   sends a `PointWatchEvent` to that consumer's UDS. aether-automation,
   aether-alarm, and aether-api cannot steal or overwrite one another's subscriptions. The event
   is a wake-up hint only; each consumer re-reads SHM, and polling repairs
   dropped events.
4. **Direct read path.** aether-alarm and aether-api resolve channel/instance
   coordinates from SQLite and re-read matching SHM slots. aether-history and aether-uplink
   preserve their configured sampling/report cadence while reading the same
   slots; events do not silently change their time-series semantics.
```
Device ──frame──► aether-io protocol adapter (decode)
                        │
                        ▼  set_direct (~10 ns/point)
                  SHM T/S slot (authoritative)
                   │             │
      per-consumer │             │ periodic sampling
      bitmap + UDS │             ├─► aether-history
       ┌───────────┴────┐        └─► aether-uplink
       ▼                ▼
 aether-automation aether-alarm/aether-api
    event hint   event hint → SHM re-read
```

## Downlink (rule/API → device)

1. An external HTTP, CLI, or MCP control call becomes a transport-neutral
   `RequestContext` in aether-automation. `ControlApplication` checks the
   `device.control` permission and explicit confirmation, persists a mandatory
   attempted audit event in local SQLite, and only then calls the command
   dispatcher. An internal deterministic rule action enters the existing
   dispatcher path directly during the staged migration.
2. The dispatcher calls aether-automation's `execute_action`
   (`services/automation/src/instance_data.rs`), which resolves the instance action point to its channel command point
   **once**, from the in-memory routing cache (a mirror of the `route:m2c`
   table populated by `aether sync`). The resolved target is threaded through
   the rest of the call so a concurrent routing reload cannot change the
   decision mid-flight.
3. The offline gate reads the channel-health SHM segment. An offline channel
   rejects the write with `ChannelUnreachable` before anything is written.
4. After value validation, `ShmDispatch`
   (`libs/aether-rtdb-shm/src/dispatch.rs`) writes the C or A slot through
   `ActionWriter::set_action`. The writer generation is checked before and
   after the write; a mismatch means aether-io restarted and rebuilt the segment,
   so the write is discarded and the dispatch fails rather than landing in a
   stale layout.
5. `ShmNotifier` sends a fixed-size 56-byte `ShmNotification` over a Unix
   domain socket
   (`libs/aether-rtdb-shm/src/notifier.rs`). The notification carries the
   channel/point coordinates, the value bits, issue/expiry timestamps, and a
   producer id + sequence number for deduplication. If aether-io is down, the
   notifier reconnects with exponential backoff (1–5 s). Native deployments
   default to `/tmp/aether-m2c.sock`; Docker sets `AETHER_M2C_SOCKET` to
   `/shm/rtdb/aether-m2c.sock` so both isolated containers see the socket.
6. aether-io's `ShmCommandListener`
   (`services/io/src/core/channels/shm_listener.rs`) receives the
   notification, rejects expired frames, deduplicates by sequence, and forwards
   a command to the owning channel's queue. Immediately before protocol
   dispatch, `CommandGuard` verifies that the writable point exists and that
   the value satisfies its min/max/step policy; only then can the protocol
   adapter write it to the field bus.

Live command data never transits a database: the transport is SHM plus the UDS
notification. Local SQLite stores security audit events around external
commands, but is not part of command delivery and never mirrors the live point
value. A dispatch that fails partway (shared memory written
but the notification lost, or no writer available) surfaces as an error to the
caller; see [Data Model](data-model.md) for how those failures map to HTTP
statuses.

## Data-processing path (source data → derived data)

Aether Data Processing introduces a third, non-authoritative path for
request-driven computation. It is neither an uplink mirror nor a downlink
command path:

```text
caller
  │ typed data-processing task
  ▼
DataProcessingApplication
  ├─ HistoryQuery ───────────── historical observations
  ├─ LiveState ──────────────── current read-only tail
  └─ task/request context ───── future or external covariates
             │
             ▼ complete, bounded ProcessingFrame
         DataProcessor
             │
             ▼ schema-validated, expiring ProcessingResult
       authenticated HTTP DerivedData response
```

The application resolves semantic bindings, aligns and aggregates timestamps,
requires commissioned unit/sign metadata to match exactly, checks missing and
stale inputs, and sends the values in the processor request. Version 1 performs
no runtime unit/sign conversion.
The processor never receives credentials for SHM, SQLite, or internal service
APIs and never resolves a site identifier by reaching back into Aether.

The result records its input watermark, input digest, processor
provenance, quality, status, and expiry. It is derived evidence rather than a
measurement: it is not written to the IO-owned T/S segment. If automation uses
the result, a separate planning/control use case validates freshness and safety
before the existing audited command path can act. Processor loss therefore
removes an optional advisory input without interrupting acquisition or local
safety rules. See [Data Processing Flow](data-processing-flow.md) for the
complete contract.

An event-time `as_of` is not by itself a historical knowledge cut. The current
historian has no ingestion/source epoch and artifact provenance has no
training/availability cut, so point-in-time evaluation uses frozen history and
artifact inputs rather than querying today's mutable sources for an old frame.

## Latency budget

The microsecond figures are measured end-to-end on production hardware
(Cortex-A55 @ 1.4 GHz, ECU-1170 / EdgeLinux 22.04) per the README, with the
full benchmark in `libs/aether-rtdb-shm/benches/BASELINE.md`. The nanosecond
figure is the README's stated order of magnitude for the hot-path write.

| Stage | Latency | Source label |
|-------|---------|--------------|
| aether-io shared-memory write (`set_direct`) | ~10 ns/point | README |
| Data change → aether-automation event received (PointWatch delivery) | P50 206 µs, P99 526 µs | README/CHANGELOG, measured |
| + rule evaluation + control SHM write + UDS notify to aether-io | ~215 µs P50, ~540 µs P99 (cumulative) | README, measured |
| + device protocol write (Modbus / IEC 104 field bus) | +5–10 ms | README |
| aether-alarm → aether-api/aether-uplink, service HTTP hops | local HTTP | — |

The CHANGELOG also records P99.9 at 1.4–2.2 ms for the event path, and notes
that PointWatch replaced the previous 100 ms Redis-tick polling model
(50–150 ms end to end) — roughly a 500× improvement on the critical path. The
software-internal control path is sub-millisecond; the field-bus write
dominates the physical control loop.

## Optional state mirrors

External state mirrors are extensions, not participants in the control path.
`extensions/redis-bridge` implements the `StateMirror` extension contract and
is built and started explicitly. It may observe SHM and publish an eventually
consistent remote view, but no default service reads from it and mirror
failure cannot affect acquisition, rules, alarms, history, API reads, uplink,
or command delivery.

The same boundary applies to other custom stores: consume SHM/events through
the extension API, keep the store non-authoritative, and do not add the store
to core service startup dependencies.

## Related pages

- [Architecture](architecture.md) — the services these paths connect
- [Shared Memory](shared-memory.md) — slot layout, seqlock, write ownership
- [Data Model](data-model.md) — points, instances, and NaN/absence semantics
- [Data Processing](data-processing.md) — the optional industry-neutral processing boundary
- [Data Processing Flow](data-processing-flow.md) — processor-request data flow and failure semantics
- [Rule Engine](rule-engine.md) — what happens after a PointWatch event arrives

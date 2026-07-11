# Aether HTTP History Query

This optional extension maps semantic Data Processing history features to a
loopback-only batch endpoint containing an already materialized cadence grid.
It only accepts the task policy `aggregation=last` and
`duplicate_policy=reject`; it does not aggregate raw observations.

Mappings are scoped by exact task and binding identity. One physical upstream
series may serve multiple tasks, while each `(task, binding, feature)` route is
unique.

The default `aether-history` collection stream is raw and therefore is not a
valid source for a resampled forecast task through this adapter. The embedded
SQLite adapter performs that production aggregation. This HTTP adapter is for
deployments whose upstream service has already materialized the exact grid and
can uphold the same contract.

The wire response is event-time aligned; it carries no ingestion cut or
source/configuration epoch. The adapter therefore does not claim point-in-time
backtest semantics unless the upstream independently freezes and attests those
cuts outside the v1 contract.

The adapter rejects redirects, credentials, query strings, fragments,
non-loopback cleartext endpoints, oversized responses, incomplete mappings,
duplicate backend points, and timestamps outside the requested half-open
window. Calendar features are generated deterministically from the requested
grid; all other features require an explicit commissioned mapping.

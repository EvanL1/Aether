# Optional processor deployment

These examples isolate the request-driven Load-Forecasting adapter from the
default Aether runtime. They are commissioning assets, not default services.

## Compose profile

Build or load an image containing the existing Load-Forecasting Edge-Platform,
the adapter from this directory's parent, and an entrypoint that serves the
composed FastAPI application on `0.0.0.0:8989` inside the isolated container.
Compose publishes that port only on host loopback. Then select the profile
explicitly:

```bash
export AETHER_LOAD_FORECASTING_IMAGE=registry.example/load-forecasting@sha256:<digest>
export AETHER_LOAD_FORECASTING_BEARER_TOKEN='<at least 32 allowed characters>'
export AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES='<strict commissioned JSON array>'
integrations/load-forecasting/deploy/validate-production-env.sh
docker compose \
  -f docker-compose.yml \
  -f integrations/load-forecasting/deploy/docker-compose.data-processing.yaml \
  --profile data-processing \
  up -d aether-load-forecasting-processor aether-api
```

The preflight script is part of the production procedure, not an optional
example. It enforces the immutable `@sha256` image form, token bounds and
character set, concurrency range, and exact artifact-bundle JSON shape before
the Compose override is evaluated.

This preflight does not attest the historian's active write target. Before
starting `aether-api`, apply any `PUT /hisApi/storage` change with a reconnect
or `aether-history` restart, verify `active_backend: sqlite`, `connected: true`,
and a known commissioned sentinel series, then ensure runtime `history.path`
matches. Persisted `history_config.storage_*` is only saved intent. Keep Data
Processing disabled throughout the storage transition.

It also does not harden the API's historian filesystem access. The base
Compose gives `aether-api` a read-write `/app/data` mount. Production must
override that layout so the historian database/WAL/SHM directory is exposed to
the API read-only under independently verified permissions, while the API's
own configuration and audit database remains separately writable. Until that
split exists, the direct-SQLite route is a blocked commissioning example even
though the adapter itself opens SQLite read-only.

Run that command from the Aether repository root. The production override is
intentional: the base file exposes only the mutable `data-processing-dev`
profile and does not evaluate production secrets or enable the Aether route.

The service is read-only, drops Linux capabilities, and receives no Aether
SHM, database, data-directory, configuration, device, or credential mount.
The endpoint and approved image digest remain deployment configuration.
The sidecar joins only the dedicated `data-processing-local` network, declared
`internal: true`, and its host port is published only on loopback. The Compose
example therefore mechanically blocks container external egress as well as
limiting inbound access. `boundary: local` remains a trust policy and does not
replace these controls. Native/systemd deployment still requires a host
firewall or equivalent egress restriction.

The examples also do not set hard CPU, memory, or PID quotas. Use the real-model
target-hardware benchmark to set container cgroup limits (or equivalent runtime
limits) and verify that overload cannot starve acquisition, history, alarms, or
deterministic automation.

Commission the sidecar with `AETHER_LOAD_FORECASTING_REQUIRE_AUTH=true`, a
secret `AETHER_LOAD_FORECASTING_BEARER_TOKEN`, a bounded
`AETHER_LOAD_FORECASTING_MAX_CONCURRENCY`, and the strict
`AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES` JSON described in the parent
README. The Aether HTTP processor adapter must send the same bearer token.
Do not commission the image until its Edge-Platform bridge proves the exact
resolved artifact paths and the upstream verbose model/scaler/prediction
prints have been removed; both are explicit readiness gates in the parent
README.

## systemd

The unit in this directory is intentionally not part of `aether.target`.
Install it only on a commissioned host after the composed Edge-Platform app is
available as `/opt/load-forecasting/app.py` and its locked `uv` environment is
at `/opt/load-forecasting/.venv`:

```bash
sudo install -m 0644 aether-load-forecasting-processor.service \
  /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now aether-load-forecasting-processor.service
```

The example binds to loopback and uses systemd sandboxing. Give a concrete
model directory read-only access with a narrowly scoped `BindReadOnlyPaths=`
override; do not grant access to Aether's SHM or data directories. Keep
`/etc/aether/load-forecasting-processor.env` owned by root with mode `0600` and
set `AETHER_LOAD_FORECASTING_REQUIRE_AUTH=true` so a missing secret prevents
startup. The unit also treats the environment file itself as required. The
health endpoints are intentionally unauthenticated and therefore
must remain on the loopback listener shown by the unit.

The unit intentionally leaves deployment-specific `CPUQuota=`, `MemoryMax=`,
`TasksMax=`, and outbound network policy unset. Add measured limits in a local
systemd override before production; loopback binding alone does not constrain
egress.

[`load-forecasting-processor.env.example`](load-forecasting-processor.env.example)
lists every supported deployment variable with non-secret placeholders. It is
not a usable production configuration until the token, digest, paths, and
version have been commissioned.

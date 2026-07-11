# AetherEMS energy gateway composition

This example proves that the AetherEMS distribution layers energy knowledge
over the industry-neutral Aether SDK without commissioning a site.

It builds the same local-only application API as `minimal-gateway`, parses the
bundled energy pack manifest and safe example configuration, and fails if a
device channel, instance auto-loading, control rule, data-processing task, or
data-processing binding is enabled. It records both disabled task assets and
strictly validates the load forecast contract while keeping processor routing
outside the pack. The PV declaration remains a disabled commissioning asset,
not a proved processor route.
`bundled_load_forecast_contract()` exposes the validated load task and its
bounded route settings without enabling or commissioning it. The local
composition test uses that exact contract with 672 stored-history samples,
known-future covariates, no live tail, and an explicitly labeled persistence
fallback.

```bash
cargo run -p aether-example-energy-gateway
cargo test -p aether-example-energy-gateway --test composition_contract
cargo test -p aether-example-energy-gateway --test data_processing_composition
```

This is a composition and conformance proof, not the six-process production
deployment. It does not connect to hardware, start a broker, or require Redis
or PostgreSQL. Its deterministic in-memory rows and fake processor are frozen
test inputs; they do not prove that today's mutable historian and model
registry can reconstruct a leakage-safe historical `as_of`.

# stado-rs

Rust implementation of the Stado job queue and compute management system for
Wisent GPU workloads: queue storage, scheduling, provider provisioning, and
agent/CLI tooling.

The port is behavior-faithful: blob layouts, JSON wire formats (including
Python `json.dumps` separators and `ensure_ascii`), version-token formats,
exit codes, and error classifications match the Python source so the two
implementations interoperate on the same buckets/containers. Where the Rust
code intentionally diverges, the reason is documented in the module docs of
the file concerned (see "Deviations" below).

## Layout (Rust module → Python source)

| Rust | Python source |
| --- | --- |
| `src/queue/mod.rs` | `stado/queue/` package; `BlobBackend` trait = the implicit backend contract; `StorageError` incl. `StorageConflict` |
| `src/queue/storage.rs` | `queue/storage.py::JobStorage` (facade + backend selection) |
| `src/queue/local_file.rs` | `queue/local_file.py::LocalFileBackend` |
| `src/queue/gcs.rs` | inline GCS SDK path of `queue/storage.py` |
| `src/queue/s3.rs` | `queue/s3.py::S3Backend` (aws-sdk-s3) |
| `src/queue/azure_blob.rs` | `queue/azure_blob.py::AzureBlobBackend` (hand-rolled Blob REST) |
| `src/queue/{submit,runs,tombstone,listing,leases,capacity,migrations}.rs` | `queue/submit.py`, `queue/runs/`, `queue/tracking/tombstone.py`, `queue/listing/`, `queue/leases/`, `queue/capacity.py`, `queue/migrations.py` |
| `src/azure_token.rs`, `src/skarbiec.rs` | Workload-identity token acquisition and the centralized Skarbiec credential-service client |
| `src/config.rs`, `src/config_file.rs` | `stado/config.py`, `stado/config_file.py` |
| `src/models.rs`, `src/constants.rs` | `stado/models.py`, `stado/constants.py` |
| `src/scheduler/` | `stado/scheduler/` (scheduler, dispatch, makespan, quota, cost, skip_done) |
| `src/providers/` | `stado/providers/` (gcp, azure, aws, vast, local, box) |
| `src/coordinator.rs`, `src/control_plane.rs` | `stado/coordinator.py`, `stado/deploy/{local,cloud}_control_plane.py` |
| `src/dashboard/` | `stado/dashboard.py`, `stado/dashboard_summary/` |
| `src/monitor/`, `src/watchdog.rs` | `stado/monitor/`, watchdog entry point |
| `src/coverage.rs` | `stado/coverage/` |
| `src/failure_fixer.rs` | `stado/failure_fixer/` |
| `src/artifacts/`, `src/artifacts_models.rs` | `stado/artifacts/` |
| `src/catalog.rs`, `src/sizing.rs`, `src/targets.rs`, `src/schedules.rs`, `src/profiles.rs` | `stado/_catalog/`, `stado/sizing/`, `stado/targets/`, `stado/schedules/`, `stado/profiles/` |
| `src/machine.rs`, `src/mcp.rs` | `stado/machine.py`, `stado/mcp/` |
| `src/autonomy/` | Cross-cloud inventory, dynamic pricing, cost allocation/forecasting, placement optimization, policy, immutable decisions, bounded reconciliation, schedules, lifecycle, and savings measurement |
| `src/cli/` | `stado/cli.py` (click → clap derive; full command tree declared, unported commands dispatch to `cli/stub.rs`) |
| `src/testutil.rs` | test-only loopback mock HTTP server |

## Build

```sh
cargo build            # debug
cargo build --release  # release
```

## Binaries

- `stado` — the CLI (`src/bin/stado.rs`): submit /
  status / results / cancel / agent / coordinator / dashboard / schedule /
  artifact / cost / quota / host / vast / …
- `stado-coverage` — coverage state reporting (`src/bin/stado_coverage.rs`).
- `stado-fix` — failure fixer (`src/bin/stado_fix.rs`).
- `stado-watchdog` — agent watchdog (`src/bin/stado_watchdog.rs`).
- `stado-mcp` — MCP server exposing the machine interface
  (`src/bin/stado_mcp.rs`).

## Tests

```sh
cargo test
```

No real cloud credentials are needed: storage-backend tests run against the
loopback mock HTTP server in `src/testutil.rs` (canned list/head/get/put
responses for every `BlobBackend` method, if-absent 409/412 mapping, ETag
CAS success + conflict, pagination, metadata merge). The S3 tests point a
real `aws-sdk-s3` client at the loopback endpoint (`endpoint_url` + static
test credentials) and assert the exact wire requests.

Full gate (all must be green):

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Autonomous control plane and FinOps

The coordinator runs one policy-driven autonomy cycle per tick. `report-only`
is the default mode: discovery, price refresh, forecasts, anomalies,
recommendations, and proposed plans are persisted, but infrastructure is not
mutated. `enforce-safe` permits only reversible schedules and placement onto
already-running capacity. `enforce-owned` additionally permits bounded new
cloud placement and may mutate only resources carrying Stado ownership or an
explicit adoption marker when the matching resource rule authorizes the
action. Production and stateful resources require per-rule
`allow_production_mutation` / `allow_stateful_mutation` exceptions in addition
to the applicable mutation permission. Incomplete inventory, stale price
coverage, unknown egress, expired decisions, failed compare-and-swap, or the
emergency pause fail closed.

Inventory covers Stado/local capacity plus GCP, AWS, and Azure compute,
storage, network, images, snapshots, reservations, and managed services.
Prices come from the GCP Cloud Billing Catalog, AWS Price List and EC2 Spot
Price History, and Azure Retail Prices APIs; no static cloud price is used by
the optimizer. The placement objective combines live compute price, startup
delay, observed retry risk, hard region/capability constraints, and RFC 3339
job deadlines. Capacity is consumed in descending priority, then
earliest-deadline order; candidates that cannot meet a deadline are rejected.
Monthly forecasts use the higher of live resource burn and observed
month-to-date billing burn.

Operator entry points:

```sh
stado optimize status --json
stado optimize policy show
stado optimize policy apply --file policy.json --expect-version VERSION
stado optimize run
stado optimize explain DECISION_ID
stado optimize pause "incident"
stado optimize resume
stado cost allocation --json
stado cost forecast --json
stado cost anomalies --json
stado cost savings --json
stado resources show --json
stado resources adopt RESOURCE_ID --owner OWNER --policy-ref POLICY_VERSION --expect-revision PROVIDER_REVISION
```

The dashboard exposes the combined queue, inventory, forecast, anomaly,
savings, and latest-decision state through `/api/state.json`; the HTML
overview renders the same control-plane summary.
Canonical state lives below `autonomy/` in the configured queue backend.
Cloud credentials are obtained only from workload identity or scoped
Skarbiec grants; process-environment credential chains are deliberately
disabled.

## Deviations

Deliberate divergences from the Python source are documented where they
live; the notable ones:

- `src/queue/mod.rs` — Python's stale `_azure_backend` attribute references
  (`capacity.py:121`, `listing/__init__.py:120`, `leases/__init__.py:143`)
  are ported as the intended single backend handle.
- `src/queue/s3.rs` — Python hand-builds and SigV4-signs the CAS PUT because
  old botocore lacked `If-Match`; aws-sdk-s3 ≥ 1.74 exposes
  `if_match`/`if_none_match` natively, so CAS/if-absent go through the SDK.
  Wire semantics (412 → `StorageConflict`, unquoted-ETag version token) are
  identical.
- `src/queue/azure_blob.rs` — no Azure SDK crate: hand-rolled Blob Storage
  REST, pinned `x-ms-version: 2023-11-03`, Bearer tokens from the shared AAD
  chain (scope `https://storage.azure.com/.default`); versioned-read 412
  surfaces as a plain error (not `StorageConflict`), matching Python's
  re-raised `ResourceModifiedError`. List Blobs XML is parsed by a small
  hand-rolled extractor (no XML crate in the dependency set).
- Version-token formats differ per backend exactly as in Python: S3 strips
  ETag quotes, Azure keeps them (quoted `"0x8D…"`); tokens are only ever
  compared against tokens from the same backend.
- `src/queue/blobinfo` analogue (`BlobInfo`) carries no bound
  download/delete closures — Rust consumers hold the `Arc<dyn BlobBackend>`.
- Further deviations are indexed in each module's doc header ("Deviation"
  notes), e.g. `src/queue/gcs.rs`, `src/coordinator.rs`,
  `src/providers/local/version_check.rs`.

## Known gaps

Conscious, verified remaining gaps (stubs print a note and exit 2, or are
documented in the module cited):

- `stado registry validate|push|pull` — CLI stub (`src/cli/mod.rs`).
- `stado host weles-recordings-dir` — CLI stub (phase-5 deploy, not part of
  this port; `src/cli/mod.rs`).
- Dashboard registry-policy endpoints `GET /api/registry.json` and
  `POST /api/registry/policy` (Python `dashboard_policy.py`) are not
  served — they fall through to 404 and the HTML policy card shows its
  safe-failure message (`src/dashboard/mod.rs`).
- Binary self-update: the version check reports a newer release but never
  downloads it (TODO(phase-4); `src/providers/local/version_check.rs`,
  `src/coordinator.rs`).
- Model policy fetch: `config/model_overrides.json` is never fetched from
  GCS; `model_policy()` always returns the empty default policy
  (`src/config.rs`).
- Capacity broadcasts omit the `vast_bridge_active` / `vast_api_key_present`
  keys (rather than reporting them as false; `src/queue/capacity.rs`).

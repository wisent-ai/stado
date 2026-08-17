# Changelog

All user-visible Stado changes are recorded here. Stado follows Semantic Versioning and the release, compatibility, migration, and rollback contract in [`docs/release.md`](docs/release.md).

## Unreleased

### Desktop

- Restored the local control plane as Stado Desktop's default source, removed
  mandatory deployment setup from the local path, and made the source visible
  in Settings.
- Kept the local operations console available without a Wisent session, moved
  account sign-in to remote deployment actions, and made the menu-bar app
  reopen the native console instead of sending users to a browser.
- Corrected dashboard state decoding and status presentation so worker,
  capacity, job, failure, and onboarding labels reflect the published backend
  snapshot instead of optimistic placeholders.
- Separated HTTPS proxy trust from deployment RLS identity: the local profile
  no longer carries an invalid non-UUID deployment binding, direct loopback
  dashboard access needs no Supabase round trip, and proxied product APIs keep
  their credential boundaries.
- Made the macOS bundle use its repository-owned canonical app icon without
  depending on a nonexistent asset resolver.
### Release control

- Added repository-owned Stado release manifests, immutable source inputs,
  signed build and delivery receipts, and provider-specific delivery adapters.
- Added fleet-wide product catalog ownership, retry-safe release submission,
  canonical promotion, exact-digest host reconciliation, and blue-green
  rollback state.
- Release-managed runtimes now receive their immutable product, version,
  platform, and artifact digest identity in the process environment.

### Onboarding platform

- Added product-scoped delivery for immutable onboarding bundles, sticky
  experiment assignment, canonical event collection, and attempt-state reads.
- Added Stado Desktop's product-owned first-use journey and gated completion on
  a real authorized job result rather than deployment or setup navigation.
- Replaced the Oko-specific onboarding relay with the same closed,
  least-privilege operation contract used by every registered product client.

### Coding clients

- Added `stado host jeden-connect` to place interactive Jeden RPC sessions on
  live registry hosts, require existing ledgers for resume placement, and carry
  the canonical bidirectional stream to native desktop clients.

### Service routing

- Directory consumer mutations now advance the routing generation atomically,
  preventing resolvers from rejecting changed directories as stale.
- Resolver adapters now close idle client streams after a bounded interval,
  preventing retained HTTP keep-alives from exhausting file descriptors and
  blocking every routed service.

### Local inference

- The documented `chat-primary` profile now uses a Featherless route for the
  same Cydonia model as its ordered fallback. With `gpu_mode=yieldable`, queued
  GPU work pauses local vLLM while Brama keeps chat available remotely.
- Route publication now accepts a temporarily stopped `yieldable` local primary
  when an ordered fallback is present; unavailable exclusive primaries and local
  fallbacks remain rejected.

### Credential recovery

- `stado credentials harvest --restore` now writes an owner-local Skarbiec vault
  through the Skarbiec CLI's field-aware contract instead of the retired
  whole-item HTTP payload. Restored values still move only over stdin and are
  never printed.

## 0.5.0-rc.1 - 2026-07-29

### Product contract

- Reframed Stado around the supported 0.5 product boundary, intended users,
  explicit non-goals, and capability-status semantics.
- Froze stable 0.5 support to local execution, local filesystem storage, and
  their provider-neutral queue, recovery, artifact, API, dashboard, MCP, and
  scoped-secret contracts.
- Declared cloud storage, cloud VM, Box, and Vast adapters preview until each
  integration has release-scoped live acceptance evidence.

### Release engineering

- Replaced the split tag-triggered publication path with one default-branch
  release/delivery run using standard `v<version>` tags.
- Unified crate licensing with the repository Apache License file.
- Defined nightly, candidate, and stable channels, immutable release manifests,
  supported platforms, compatibility rules, and upgrade/rollback gates.

### Onboarding

- Added a no-argument first-run path and a minimal local configuration contract.
- Removed cloud credentials, product-specific API clients, and optional
  integrations from the required local onboarding path.
- Documented enrollment as the verified path it is: `stado_fleet key generate`
  prints the public key that first contact needs, `stado_fleet enroll` probes the
  machine's hostname and platform before writing them, and `stado registry host
  add` is the declaration on its own. The documented `host add` invocations were
  missing the required `--release-platform` and failed as written.
- Gave `stado_fleet` a build and install path of its own. Having none is how this
  control plane came to run `stado_fleet` 0.5.1 against `stado` 0.7.2 from one
  shared library, until `stado_fleet key ls` began answering HTTP 400 against the
  current Skarbiec field-read contract. `install-built-stado-binary.py` now also
  accepts a repair: where the running binary fails a read-only probe the
  candidate passes, agreement with the broken binary is not required.

### Credentials

- Restored every write into a Skarbiec-backed credential store. `PUT /v1/items`
  became the Weles acquisition route when the vault contracts were rebuilt, and
  it requires `id`, `field` and `operation_id` and refuses an item it does not
  control; Stado still sent whole items, so `stado credentials put`,
  `stado_fleet key generate|add|rotate` and the Azure operator credential all
  answered `400 {"error":"field required"}`. The fleet could read credentials and
  could not mint one, so no new host could be enrolled. Writes and deletes now go
  through the vault's owner, in one place inside `credential_store`, instead of
  one command knowing the contract and the rest guessing.
- Named Skarbiec's canonical kinds and its field/context split where Stado writes
  them: a host key is a `key-pair` with the two halves as fields and its
  fingerprint and key type as context, and the Azure operator session is a
  `stado-secret` rather than an `oauth-client` that allows only two fields.
  `stado_fleet key ls` reads that context instead of printing two blank columns,
  and `key generate` reads the new item back through the same client the SSH
  channel uses — an owner write reaches a vault file while every consumer reaches
  a broker, and on a host whose broker forwards to another machine's vault those
  are different stores.

### Core behavior

- Added durable cancelled records and canonical lifecycle reconciliation.
- Replaced the global Python preflight and CUDA probe with runtime-scoped,
  native checks; optional Hugging Face cleanup remains isolated.
- Hardened the agent loop, public machine contract, results/artifact handling,
  secret redaction, pause/drain recovery, and versioned config/storage schemas.

### Integrations

- Stabilized the shared storage contract for local filesystem, GCS, S3, and
  Azure Blob, including conditional writes, listing, metadata, and recovery.
- Kept GCS, S3, Azure Blob, GCE, EC2, Azure VM, Box, and Vast as preview.
  Current live attempts reached provider APIs but could not qualify them:
  GCP billing is disabled, the available AWS access key is invalid, and no
  Azure managed-identity sandbox is provisioned.

### Verification

- The current Rust tree passes 697 tests across all targets and features,
  with four provider-live suites ignored by default.
- Clippy passes with warnings denied and rustfmt reports no differences.

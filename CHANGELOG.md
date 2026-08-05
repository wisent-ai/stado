# Changelog

All user-visible Stado changes are recorded here. Stado follows Semantic Versioning and the release, compatibility, migration, and rollback contract in [`docs/release.md`](docs/release.md).

## Unreleased

### Onboarding platform

- Added product-scoped delivery for immutable onboarding bundles, sticky
  experiment assignment, canonical event collection, and attempt-state reads.
- Added Stado Desktop's product-owned first-use journey and gated completion on
  a real authorized job result rather than deployment or setup navigation.
- Replaced the Oko-specific onboarding relay with the same closed,
  least-privilege operation contract used by every registered product client.

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

- Changed production publication from default-branch pushes to explicit
  `stado-v*` tags or manual dispatch.
- Unified crate licensing with the repository Apache License file.
- Defined nightly, candidate, and stable channels, immutable release manifests,
  supported platforms, compatibility rules, and upgrade/rollback gates.

### Onboarding

- Added a no-argument first-run path and a minimal local configuration contract.
- Removed cloud credentials, product-specific API clients, and optional
  integrations from the required local onboarding path.

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

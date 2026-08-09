# Release and Compatibility

This document defines how a Stado source revision becomes an identifiable, immutable, upgradeable, and recoverable product release.

## Canonical version

`stado-rs/Cargo.toml` is the only source of the product version. Binaries expose that version through Clap, release jobs read it from the manifest, and every published object uses it in its immutable coordinate.

A release tag has the form:

```text
v<semver>
```

The tag version must equal the Cargo package version exactly after removing the `v` prefix.

Stado follows Semantic Versioning:

- patch: compatible defect, security, and operational corrections;
- minor: backward-compatible public capabilities or optional fields;
- major: an incompatible CLI, machine API, persisted-state, configuration, or provider contract.

Before 1.0, an incompatible preview-integration change may occur in a minor release only when the release notes identify the affected adapter, migration, and rollback boundary.

## Channels

| Channel | Source | Purpose | Production deployment |
|---|---|---|---|
| `nightly` | scheduled build from the default branch | development evidence; never an upgrade target | prohibited |
| `candidate` | prerelease such as `v0.5.0-rc.1` | canary, compatibility, and rollback acceptance | canary only |
| `stable` | final version such as `v0.5.0` | operator-approved fleet release | explicit promotion only |

Channels are discovery metadata. Runtime installation always resolves to and pins an exact immutable version and platform; no host executes a mutable `latest` object.

## Supported release matrix

The release manifest is authoritative. The initial product matrix is:

| Platform coordinate | Role | Channel eligibility |
|---|---|---|
| `macos-arm64` | control plane and local agent | candidate and stable |
| `linux-amd64` | control plane and local/cloud agent | candidate and stable |
| `linux-arm64` | none | nightly experimentation only; unsupported until promoted |

A platform is promoted only after its clean-install, first-workload, upgrade, and rollback evidence is attached to the candidate release.

### Frozen Stado 0.5 support scope

The stable 0.5 integration set is local compute on an attached host, local
filesystem storage, and the provider-neutral queue, lifecycle, recovery,
artifact, machine API, dashboard, MCP, and scoped-Skarbiec contracts exercised
by that path. GCS, S3, Azure Blob, GCE, EC2, Azure VM, Box, and Vast adapters
ship as preview integrations. A failed or unavailable preview-provider live
suite blocks only promotion of that integration, not the local 0.5 release,
unless it exposes a shared queue, security, or recovery defect.

Promotion reads the canonical archive manifests and never infers platform
support from an implemented capability.

## Immutable artifact layout

```text
stado://releases/stado/<version>/<platform>/stado-v<version>-<platform>.tar.gz
stado://releases/stado/<version>/<platform>/release-manifest-<platform>.json
```

Publication is create-if-absent. Republishing identical bytes is idempotent. A different body at an existing coordinate is an immutable-release collision and fails.

## Release manifest

Each platform manifest contains exactly:

- `product`;
- `version`;
- `platform`;
- `source_commit`;
- `sha256`, the digest of the adjacent release archive.

The Stado and Skarbiec publishers use the same archive/manifest shape. Delivery
verifies the archive digest, extracts the fixed managed-binary member, verifies
its reported version, and only then atomically activates it.

## Build and publication identity

A release build must:

1. check out the committed source revision;
2. build each supported platform once with the locked dependency graph;
3. record the exact source commit and archive digest;
4. perform the CLI-surface version check against the committed Cargo version;
5. create or safely resume `v<version>` at that same commit;
6. publish immutable archive and manifest assets;
7. bootstrap the control plane from those built bytes;
8. promote desired state through the fenced registry writer and reconcile.

Build, release-publisher, runtime, dashboard, object-client, and workload credentials remain separate.

## Compatibility matrix

| Contract | Compatibility rule |
|---|---|
| Human CLI | flags and commands may be added compatibly; removal or semantic reversal requires a major release |
| Machine JSON | clients send and receive a supported `schema_version`; additive optional fields are compatible |
| MCP | protocol version and tool schemas are explicit; mutation authority is never added to the read-only server implicitly |
| Job JSON | unknown additive fields are preserved where round-trip ownership requires it; incompatible required fields require migration |
| Queue/storage layout | readers reject unsupported future schema versions; writers never silently downgrade canonical state |
| Configuration | the root schema version is required; migration produces a new document and preserves the prior file for rollback |
| Coordinator/agent | a release manifest declares the minimum compatible agent; dispatch refuses an incompatible agent |
| Provider adapters | implemented capability does not imply stable support; the released capability catalog and live evidence define support |

## Upgrade procedure

1. Read release notes and compatibility range.
2. Record current exact version and platform.
3. Run `stado doctor --fix-hints`.
4. Pause new work and drain running work when the release changes state or execution contracts.
5. Verify the configured backup destination.
6. Copy and verify canonical state when a schema migration requires a recovery point.
7. Download the exact candidate through the public release route.
8. Verify manifest identity and SHA-256.
9. Install atomically while preserving the prior binary.
10. Restart the selected canary service.
11. Verify version, health, queue visibility, and one representative workload.
12. Resume dispatch only after the candidate evidence is clean.

## Rollback procedure

Rollback uses the exact previously recorded release coordinate; it never rebuilds source in place.

1. Pause dispatch.
2. Stop the affected service.
3. Restore the previous verified binary atomically.
4. Restore the previous configuration file if configuration migration occurred.
5. Restore canonical state only when release notes explicitly state that the new writer produced an incompatible layout.
6. Restart and verify version, storage reachability, queue counts, and agent compatibility.
7. Resume after one successful local workload.

If a migration is forward-only, the release notes must say so before installation and the rollback procedure must restore a pre-migration state copy.

## Release notes contract

Every candidate and stable release records:

- source commit and immutable artifact coordinates;
- user-visible changes;
- fixed failures and security changes;
- CLI and machine-contract changes;
- configuration and persisted-state migrations;
- provider capability changes;
- supported platforms;
- known limitations;
- required operator actions;
- upgrade and rollback instructions;
- acceptance evidence and exclusions.

User-visible history lives in [`CHANGELOG.md`](../CHANGELOG.md). Incident detail belongs under `docs/incidents/`, not in the changelog.

## Promotion gate

A candidate may become stable only when:

- the version/tag/source relationship is exact;
- all required platform artifacts and manifests exist;
- checksums verify;
- onboarding succeeds from a clean supported environment;
- core contract suites pass;
- every stable integration has current live acceptance evidence;
- upgrade and rollback succeed on a canary;
- release notes describe all compatibility and migration effects;
- an operator explicitly promotes the immutable candidate.

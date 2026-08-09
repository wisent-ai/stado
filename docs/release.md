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

## Stado-native release pipeline

Every product carries one strict `.wisent-release.json` v1 contract. A product
that is not released declares only `schema_version`, `product`,
`releases:false`, and a reason. A releasing product declares its version source,
platform recipes, argv-array quality and build commands, stage mapping,
optional runtime contract, promotion policy, immutable inputs, deliveries, and
provider-neutral mirrors. Repository URLs, branches, hosts, buckets, tokens,
signing-key bytes, and provider credentials are forbidden from that file.

The product version is selected by AutoVersion and passed explicitly:

```text
stado release submit --source DIR --version V --channel candidate
```

Stado verifies `V` against the manifest's checked-in `json`, `regex`, or `text`
version source. It requires a clean committed tree and never contacts a Git
remote.

The ownership chain is:

```text
committed tree
  -> deterministic source.tar.gz
  -> stado://sources/<product>/<sha256>/source.tar.gz
  -> fleet queue quality/build job per platform
  -> status/<job>/output/{receipt.json,release.tar.gz}
  -> signed stado://releases/<product>/<version>/<platform>/
  -> registry.release_control desired CAS
  -> release_agent reconciliation
  -> exact-digest deployment receipt
```

The source object is create-only and carries metadata for its exact Git commit,
source digest, and pipeline-manifest digest. A durable
`stado://release-runs/<id>/run.json` joins those identities to platform job IDs,
canonical output prefixes, delivery jobs, state, and failure. Repeating submit
with the same inputs resumes that run and does not rebuild a platform whose
published output is already recorded.

Platform output coordinates are canonical lowercase identifiers. Each recipe
names the verified fleet `runner_platform` (`darwin-arm64` or `linux-amd64`),
quality argv in order, one build argv, staged source-to-archive paths, and
optional `secret_env` references of the form `ENV=item#field`. Builders receive
no repository coordinate or repository token. They materialize only the exact
source URI and declared immutable inputs, extract inputs under
`WISENT_INPUTS_DIR`, and write output through the queue's existing canonical
job-output collection.

Publication writes the archive, immutable qualification receipt, signature,
and signed manifest last. The signing key is read from the configured Skarbiec
item field `private_key`; only the item name and trusted key ID are
configuration. For the default configuration the item is
`stado-release-signing`, never a secret value in a manifest or command line.

Post-publication deliveries are queue jobs consuming the canonical archive URI
and digest. Required package or product channels gate completion. Optional
mirrors are recorded but do not determine canonical release success. No
GitHub-hosted step is part of the source, qualification, publication,
promotion, or reconciliation contract.

`promotion.reconcile:true` is reserved for registry-hosted runtime products and
requires a runtime contract. Package, web, mobile, source, and archive products
use `reconcile:false`; they may omit runtime fields and complete through their
required delivery receipts.

## Product catalog

Stado owns product release policy independently of repository hosting:

```text
stado release catalog sync --root ROOT
stado release catalog audit
```

Sync reads local registered checkouts, including explicit `releases:false`
manifests, refuses duplicate product names, and CAS-updates
`stado://system/release-catalog/<product>.json`. Submit records the strict
manifest together with its immutable source identity before queueing work.
Audit reads only Stado catalog objects and refuses malformed, duplicate, or
silent catalogs; it does not enumerate a Git forge or require forge tokens.

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

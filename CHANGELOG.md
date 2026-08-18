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
- Added an Add a Machine path to Stado Desktop's Hosts screen, reachable from
  the context bar and from the empty registry state. It walks naming, key
  minting with the public half and its authorized_keys line to carry,
  the SSH address, verified enrollment, and the channel and agent proofs;
  every step runs one allowlisted `fleet` or `host` command through the
  dashboard's `POST /api/operator/run` argv bridge, never a shell string.
  Progress survives closing the window, so the walk to the other machine
  does not cost the minted key, and an enrollment that fails says whether it
  never reached the machine or reached it and rolled its own entry back.
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
- Corrected and completed the machine-onboarding documentation. Every
  documented `stado registry host add` invocation now carries the required
  `--release-platform` alongside `--ssh`; the previously published form failed
  on use.
- Documented enrollment as the verified path it is: `stado fleet key generate`
  prints the public key that first contact needs, `stado fleet key install`
  travels through the existing channel and is therefore rotation rather than
  first contact, and `stado fleet enroll` probes `hostname` and `uname` before
  it writes and rolls the entry back when bootstrap fails. Added the
  `stado fleet` family to the CLI reference and recorded Stado Desktop's
  equivalent Add-a-Machine surface; `stado_fleet` is documented only as a
  compatibility binary.
- Documented that the SSH destination may be any reachable target — a `.local`
  name on the local network is as valid as a tailnet name — and what a `.local`
  destination costs: channel-opening commands then require the same network,
  while the outward health beacon keeps `stado registry beacon-age` reporting.
- Added [Add your own machine](docs/add-your-machine.md) for the owner of a
  machine joining someone else's fleet, linked from the README next to the
  operator onboarding path.
- Added `deploy/join.sh`, the one-line bootstrap the owner of a joining machine
  runs for the `invite` method: `curl -fsSL <control-url>/join.sh | sh -s --
  <code>`. It redeems the invitation for the fleet's public half, installs that
  line into `~/.ssh/authorized_keys` idempotently with 700/600 modes, resolves
  the address the fleet should dial (tailnet name, then `.local`, then the
  default interface's IPv4), and reports the machine as a pending enrollment.
  It never handles a private key, never prints or stores the invitation code,
  never enables Remote Login silently — it diagnoses SSH and prints the exact
  macOS or Linux step for the owner — and deliberately installs no agent, since
  the operator's `stado fleet approve` does that over the channel it just
  opened.
- Rewrote the machine-adding documentation around the four named methods
  instead of one procedure: [Onboard another machine](docs/onboarding.md#onboard-another-machine)
  now opens with a chooser table for `invite`, `adopt`, `join` and `declare` —
  what each needs from the operator, what it needs from the machine, when it is
  the right one, and what it cannot do — and gives each method its own section.
  Every method states the same checkable property: the private half of the
  channel key never leaves the operator's credential store, and the machine
  receives only the public line.
- [Add your own machine](docs/add-your-machine.md) now leads with the one line
  the owner of a joining machine actually runs, because the invitation is the
  normal path; pasting a key by hand is kept below as the route for a machine
  that cannot run it, next to the operator-driven `adopt` alternative.
- Documented in the CLI reference: `stado fleet methods`, `stado fleet invite`,
  `stado fleet invites`, `stado fleet revoke-invite`, `--install-key` on
  `stado fleet enroll`, `--json` on `stado fleet pending`, the `allow_invite`
  and `allow_adopt` catalog fields, and the three invite routes
  (`GET /api/fleet/invite/key`, `POST /api/fleet/join`, `GET /join.sh`) —
  authorized by an invitation token alone, and unable to write the registry,
  which stays an operator-authority write inside `stado fleet approve`.
- Added [`docs/examples/fleet/invite-a-machine.sh`](docs/examples/fleet/invite-a-machine.sh),
  the `invite` method end to end from the operator's side, from `fleet invite`
  to `fleet approve` and `registry beacon-age` as the proof, indexed in the
  examples README.
- Served the `invite` method from the dashboard: `GET /api/fleet/invite/key`
  hands the joining machine the fleet's public half and the exact
  `authorized_keys` line, `POST /api/fleet/join` files its pending enrollment
  request and spends one use of the invitation, and `GET /join.sh` serves the
  repository's bootstrap script verbatim and uncached. The two API routes are
  authorized by the invitation token alone — never by operator credentials,
  and not by the implicit trust a loopback caller has on operator routes — and
  write nothing outside `enrollments/`. Unknown, wrong, spent, revoked,
  expired and rate-limited codes all answer with one status, one sentence and
  the same elapsed time, so the routes cannot be used to enumerate or classify
  invitations; requests are bounded per code, per address and in size before
  any credential store or object store is read.

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
- Minting an SSH host key now ends with a key that can actually be read. Skarbiec
  authorizes reads per item, so a freshly written key was readable by nobody: the
  consumer every host channel authenticates as gained no capability from the
  write, and every new key was dead until an operator widened that grant by hand.
  `key generate` and `key add` now widen it themselves — preserving the
  consumer's bearer, its remaining lifetime, and every capability it already
  held — and prove the result by reading the item back through the same consumer
  the channel uses. A read-back that returns a different value still fails the
  mint: it means the broker serves a vault this machine's write never reached.
- `stado fleet enroll NAME --ssh DEST --install-key` adopts a machine that is
  not in the fleet yet. Enrolling presupposed that the fleet's public key was
  already in the machine's `authorized_keys`, because both the identity probe
  and `fleet key install` open the channel with the vault key itself — so
  adding someone's laptop began with an operator dictating a key over the
  phone. The flag installs it over a session the operator can already open
  otherwise: a loaded or forwarded ssh agent, one of their own keys, or
  OpenSSH's own password prompt, which OpenSSH asks and answers on its own tty.
  Stado never sees a password, the private half never leaves the operator's
  vault, and the line travels on stdin rather than in argv. A pair is minted
  through the existing `key generate` if the target has none, the append is
  skipped when the exact line is already present, and the run then continues
  down the unchanged path — probe the hostname and platform before the registry
  write, roll the entry back on a failed `--bootstrap`. The three ways first
  contact can fail now read as three different sentences, because they need
  three different actions: no connection was established, the connection was
  established and the credential rejected, or the credential worked and the
  machine's home directory refused the write. `registry.enrollment.allow_adopt`
  gates it.

### Core behavior

- The disk cleaner has a third cleaner, `build_caches`, so the automatic pass
  can reclaim build output. It knew only `huggingface_cache` and
  `weles_recordings`, which is why an operator laptop reached 8.8 GB free of
  1.8 TB — roughly 450 GB of build and scratch trees — while `disk-cleanup`
  had nothing to report. A directory is removed only when it carries a
  `CACHEDIR.TAG` whose first line is the Cache Directory Tagging Standard
  signature, the same criterion `stado host build-caches` already applied on
  request; no directory names or extensions are matched. Its policy takes
  `min_age_seconds` (at least 86400) and an optional `root`, defaulting to the
  host's `$HOME`, and it reports under `build_caches` like the other two.

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
- Made enrollment part of `stado`: adding a machine is `stado fleet enroll`, with
  `join`/`pending`/`approve`/`reject`, `key generate|install|check|rotate|ls|rm|add`,
  `list`, `status`, `create`, `assign`, `catalog` and `doctor` beside it. The
  dashboard's operator console can now run enrollment, which it never could: it
  executes `stado`, and enrollment existed only inside the separate `stado_fleet`
  binary, so the first command a new machine needs was absent from `stado --help`
  and from every surface built on it. `stado_fleet` keeps every command, flag and
  word of output, now as a thin entry point onto the same library code — there is
  one implementation, not two. Both `stado_fleet` and `stado_migrate` are also
  declared in the crate manifest instead of being found by directory: nothing
  naming them is what let `stado_fleet` run 0.5.1 against the 0.7.2 library it
  shares with `stado` for weeks, with no command able to report the gap.

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

# Native builds

Stado stores native build recipes in the canonical registry. A recipe names a public HTTPS Git repository, a branch, one POSIX shell command, the artifact paths produced by that command, and one or more worker platforms. The coordinator watches the branch and enqueues one job per platform when its head changes. A worker can claim only the job for its own platform.

Builds and releases are separate. A build uploads the declared artifacts and may record the exact semantic-version tag found on the commit. It does not sign an artifact, change `release_control`, or promote a release. `stado release submit` owns qualification, signing through Skarbiec, publication, delivery, and installation.
When Stado itself is delivered, the installer records the installed version
beside the managed binary. Each managed queue agent finishes its active jobs,
detects that its loaded version differs, and exits through its declared
`KeepAlive` or `Restart=on-failure` policy; the supervisor then starts the
installed release without unloading the unit or interrupting a job.
Before executing an artifact, every delivery verifies both the newest submitted
source in the product catalog and the exact published platform coordinate in
its still-delivering run. This fences an older queued delivery without assuming
the product has a separate release-control rollout policy.
The Stado delivery job starts its worker from the digest-pinned candidate
archive and that worker uses itself for `install-local`; a broken older
installed worker therefore cannot prevent the release that repairs it.
A non-Stado archive contains that product rather than another Stado binary, so
its delivery runs the installed Stado worker against the digest-pinned archive.

Repeating `stado release submit` resumes the same release run. An initial
platform submission keeps its original stable job identity and output URI. If
that platform is already recorded as failed, the coordinator derives one retry
identity from the release run, platform, and prior terminal job ID, and gives
that attempt its own output URI. A crash before or after the replacement
platform record is saved therefore reuses the same attempt; only a newly
persisted terminal failure can chain to another retry.

Local queue jobs execute from the agent owner's
`~/.stado/work/jobs/wc-<job-id>` tree, not a temporary directory. Admission
resolves the physical home, then creates or opens `.stado`, `work`, and `jobs`
one component at a time as owner-only directories with
`O_DIRECTORY|O_NOFOLLOW`; component symlinks are refused. The serialized
old-agent bridge repeats that refusal and changes permissions or
creates the next child only from a held cwd whose physical path it just
verified.
A release submission remains able to repair an older agent: its deterministic
bootstrap moves that agent's already-materialized job into the persistent root
before the worker starts and preserves a compatibility symlink for the old
agent's log and artifact upload. It also pins `TMPDIR`, `TMP`, and `TEMP`
beneath that persistent tree so a pre-migration worker cannot put its release
scratch back in OS temp.
After a successful worker, the bridge uses the checked storage writer to upload
and read back the command log and archive at both canonical status and exact
attempt URIs, then publishes the attempt's `receipt.json` last as the completion
marker. Each writer receipt must match the quiescent local source's SHA-256 and
byte count. Its JSON and lifecycle-watch responses use random 0600 files under
the validated work tmp directory, are removed after parsing, and leave only
their proof lines in the already-open command log. A failed write or read-back
turns the workload into a failure instead of allowing a successful terminal
record.
For a worker that already failed, the bridge still proves the command log at
both destinations and, when an owned regular `receipt.json` exists, publishes
that failed receipt to canonical then attempt storage. Evidence errors are
appended to the safe log without replacing the worker's original nonzero exit
code; the legacy link remains for the old agent's canonical finalization.
The bootstrap prints both physical workdir and effective temporary root before
invoking the worker, so an operator can prove the retry left `/tmp` from the
canonical job log. Queue-workdir reclamation runs only through the janitor under
its exclusive admission lock; every pass reserves a bounded scan share for
terminal old-agent links independently of canonical workdir enumeration.
If the canonical tree is ever absent or replaced while a job is live, the agent
emits `workdir_missing` with its exact expected path at heartbeat and
finalization, then retains that marker and the real workload exit code in the
terminal job error. An existing empty command log remains the distinct
`wrote no output` case.

`stado release redeliver PRODUCT RUN_ID DELIVERY --retry-token TOKEN` is the
operator recovery path for one delivery from the exact newest completed run. It
does not publish a new candidate or move a channel. The retry token identifies
a fixed per-run CAS transaction and stable queue submission, so repeating the
same command resumes after interruption. A different token is refused while a
transaction is active. Success replaces that delivery's job, output, state, and
receipt evidence before restoring the run's prior state; failure preserves the
previous passed delivery while recording the terminal failure and restoring the
run state.

`stado host config-set TARGET KEY VALUE` migrates an older deployment profile
through the installed `stado config migrate` before applying the field.
The migration preserves the exact prior file beside the profile and refuses
newer schemas. No separate migration command is needed;
`--reload-service SERVICE` activates the change through the declared service policy.

## Release publication authority

The publisher command reads the bearer named by `release_api.publishers` through
its selected credential store and configured `WC_SKARBIEC_CONSUMER` grant.
When owner credentials are available, Stado adds only that item's `token` read
to the same grant before reading it; an already authorized read does not depend
on that repair being available.

The release server uses `release_api.skarbiec.url` and its separate
`stado-release-api-verifier` grant to verify the publisher bearer. Release
signing uses the same declared authority with `stado-release-coordinator`,
which reads only the signing key. The publisher command never authenticates
its credential read as the server verifier, and `STADO_API_TOKEN` remains a
generic object API credential, never a product release bearer.

The public release route accepts exactly one `uri` query field. Exact
`GET /api/release/object?uri=stado://releases/...` reads and byte ranges are
public so a damaged credential plane cannot make signed recovery artifacts
unreachable; duplicate `uri`, `versioned`, and unknown query fields are
rejected before storage access. Release writes and exact release listings
remain authenticated by the product publisher contract above and do not
require the generic object credential first.

Before any platform artifact is written, every publisher create-only claims
`stado://releases/<product>/<version>/source-revision.json`. That version record
is the single arbitration point across changing platform sets; platform claim
records are written only after it agrees. Older platform-only coordinates are
backfilled only when every existing platform claim names the same full source
commit. Release workers set both `WISENT_SOURCE_COMMIT` and
`STADO_SOURCE_REVISION` from that verified request, and the Stado build rejects
missing-shape or mismatched overrides.

The object API recovery helper treats a release target's legacy launchd plist
and label as an optional pair. A healthy handoff or a host with no exact orphan
does not need that pair. Recovery validates and bootstraps it only when stopping
an exact dead release-proxy orphan; without a declared pair, that branch refuses
before sending `TERM` because it has no known service to restore.
Only the pair on the release target authorizes that restore; a similarly named
entry under `targets[].services` is never inferred as a fallback.

## Wait for the stable release endpoint

A blue-green candidate is not active merely because its own port answers.
Stado also checks the stable proxy's process, immutable release identity,
generation, upstream port, and declared readiness endpoint before routing is
reported as complete. Activation, reconciliation, and rollback allow that
endpoint to become ready within the product's
`strategy.readiness_timeout_seconds`; they do not assume a newly spawned proxy
has bound its socket after a fixed delay.

An exited proxy fails immediately. When the deadline expires, the refusal names
the stable URL, allowed seconds, and last connection error or HTTP status.
Identity and generation mismatches still fail without waiting.

Read the proxy's own output separately from the candidate's output:

```console
stado release logs brama --target charless-mac-mini --version proxy --stream both --json
stado release logs brama --target charless-mac-mini --version 0.2.69 --stream both --json
stado release quarantine list brama --target charless-mac-mini --json
```

After repairing the recorded cause, `release quarantine clear` retires only the
exact digest named by `--digest` and records the required `--reason`. It does not
rebuild or replace the signed release.

## Resolve the executable that is actually active

```console
stado release active-binary skarbiec --json
```

`release active-binary` resolves the local registry target unless `--target`
names that same host. It reads observed rollout state, not `policy.desired`: a
newer quarantined candidate cannot displace the healthy predecessor that still
serves the stable bind. The command succeeds only when one live release process
matches the recorded PID, version, candidate port, and immutable release
directory; the exact Stado proxy executable and argument vector target that
port at the recorded generation; and the installed marker, qualification,
artifact digest, manifest digest, platform, directory, and executable all
agree. The executable must be a regular, non-symlink, executable file.

The policy-derived path uses the same canonical helper as the release agent:
`{home}` expands to the target's declared home, and a relative install root is
resolved beneath that home before the immutable release directory is appended.

JSON output includes `state`, `product`, `target`, `version`, `platform`,
`artifact_sha256`, `manifest_sha256`, and the absolute `path`; human output is
the path alone for direct command substitution. If release control declares
the product and target but no release is observably active, the command fails
instead of falling back to an old file. Stado's authenticator-seed freshness
reader and Weles use this same contract for `SKARBIEC_BIN`, so they cannot
select desired, quarantined, or merely present Skarbiec bytes independently.

## Create and run a recipe

Recipes start disabled. This prevents the coordinator from polling an incomplete or newly reviewed definition.

```console
stado builds add \
  --name weles-native \
  --repo https://github.com/wisent-ai/weles.git \
  --branch main \
  --command 'cargo build --locked --release' \
  --artifact target/release/weles \
  --platform darwin-arm64 \
  --platform linux-amd64

stado builds enable weles-native
```

`--artifact` and `--platform` are repeatable. `--interval-seconds` changes the default 300-second polling interval. `--auto-declare` records the version from an exact semantic-version Git tag on hosts of the matching platform; it still does not promote a release.

To enqueue all platform jobs immediately instead of waiting for the next branch change:

```console
stado builds run weles-native --run-id operator-ticket-1234 --json
```

The caller retains `--run-id`: retrying the same token after a crash recovers the same per-platform durable manifests instead of creating more jobs. A distinct intentional run needs a distinct token. The command returns one job ID per platform. The worker clones the repository at the recorded ref, runs the recipe command inside that checkout, and uploads only the declared artifact paths to the normal Stado results store.

## Inspect and change recipes

```console
stado builds list --json
stado builds status weles-native --json
stado builds edit weles-native --interval-seconds 60
stado builds disable weles-native
stado builds remove weles-native
```

`status` reports the recipe, its last observed Git ref, each platform run, and the current Stado job state. `edit` changes only the supplied fields. Supplying `--artifact` or `--platform` replaces that complete list. Changing the repository or branch clears the previous ref and recorded runs. Disabling a recipe stops new polling; it does not cancel jobs already submitted.

Download a completed job's artifacts with the regular results command:

```console
stado results <job-id> ./build-results
```

A recipe accepts only an `https://` clone URL. Credentials do not belong in the URL or command. Build-time secrets use Stado workload secret references and host-local Skarbiec grants, so values do not enter the recipe, process arguments, or registry.

## Failure boundaries

| State | Meaning |
|---|---|
| no run for a platform | no job has been submitted for that platform |
| queued | no matching worker has claimed the job yet |
| running | a matching worker claimed the job |
| failed | clone, command execution, or artifact upload failed |
| succeeded | every declared artifact was uploaded and is readable from Stado results |

A successful process exit without every declared artifact is a failed build. A host merely declaring a platform is insufficient: a live worker must publish capacity for that platform.

## The quality gate names whose revision refused

`.wisent-release.json` declares `platforms.<platform>.quality`: the argv the
release worker executes before it builds. `scripts/quality_gate.sh` reads that
same key and runs the same argv, so a pull request is judged by the gate the
release stands behind, with one declaration used twice.

A pull request is judged on its **merge result**, so a step that already
refuses the base branch refuses every pull request opened against it — for
files the author never touched. On a refusal the script therefore re-runs that
one step against the base revision in its own `git worktree` and reports:

| Verdict | Meaning | Who repairs it |
|---|---|---|
| `verdict=introduced` | the step passes on the base and refuses this revision | this change |
| `verdict=inherited` | the step already refuses the base, at the printed sha | the base, in its own change |
| `verdict=unattributed` | the base could not be checked out, or a clean tree is being compared against its own commit, so ownership is unknown | read the printed step and decide |

The base is the pull request's base sha, and on a push to `main` the commit
before that push. Comparing `main` against `origin/main` compares the revision
under judgement against itself, which would answer `inherited` for every
failure and never name the push that caused it; the script refuses that as
evidence and says so.

Running it by hand on a checkout with an uncommitted break still attributes:
the base sha equals `HEAD`, but the base's own worktree does not carry the
break, so the comparison is real and the answer is `introduced`.

Every verdict still fails the gate: an inherited failure is a failure, and the
release would hit it with a version already spent. What changes is that the
message no longer sends the wrong author to reformat somebody else's code.

Run it by hand exactly as CI does:

```console
$ bash scripts/quality_gate.sh --base origin/main
platform: darwin-arm64
.wisent-release.json declares 3 quality step(s) for darwin-arm64
```

`--manifest` reads a different manifest and `--help` prints this contract. An
absent platform, an absent `quality` key or an empty one is a refusal, not a
pass with zero steps.

This exists because on 2026-09-04 and 2026-09-05 `main` carried unformatted
`cli/onboarding.rs`, then `cli/identity.rs`, then `dashboard/mod.rs`. Each one
turned every open pull request red, the gate said "fix the tree" to authors who
had touched none of those files, and three of them reformatted another
revision's code to get their own work through.

## Verify every supported platform

The release platform matrix runs both real journeys on the fleet's macOS ARM64 and Linux AMD64 workers. It checks out one exact public commit on each host, uses a real host-local Skarbiec binary for the isolated signing grant, then runs the native-build and complete release journeys.

For an online host reachable through Stado's managed host channel:

```console
stado host verify-release-platform charless-mac-mini \
  --repo https://github.com/wisent-ai/stado.git \
  --ref <full-lowercase-commit> \
  --json
```

The command accepts only a public HTTPS repository and a full 40-character lowercase commit. Source is cloned into the host's managed `~/.stado/work` area and removed when the run ends. A platform passes only when the build artifact is downloaded and verified and the signed release is published, installed, and executed on that same platform.

Probierz owns the combined `platform-matrix` journey in `stado-rs/tests/platform-matrix/`. It runs macOS through the managed host channel and submits Linux to the pinned `local-ubuntu-server` worker through the normal Stado queue, so an inbound SSH port is not a requirement. The Linux worker verifies the digest of the published Skarbiec binary before using it and keeps Cargo output inside that job's `.wisent-output` tree, covered by terminal-job cleanup. The journey runs the platforms one after another because both release checks use the same canonical test product and version.
The matrix also cancels a release build and proves that its replacement uses a
different job, then publishes, installs, and executes the product on each
platform. Linux submissions use a stable Probierz-derived run ID, and Probierz
retains the submission identity and complete terminal job report with its logs.
Both paths use the published, digest-pinned Skarbiec 0.1.3 binary rather than
rebuilding a moving dependency branch, and keep temporary files in managed work.
Disposable qualification builds omit debug symbols and incremental caches;
runtime checks and disk admission thresholds remain unchanged. The Linux
journey removes its own Cargo output on success or failure and keeps the
downloaded signing tool in ignored work files, so the recorded source stays
clean.

For disk pressure, `stado host disk TARGET --json` includes Linux inventory
under the managed home, `/home`, `/mnt`, `/var`, and `/opt`.
`stado host reclaim TARGET --apply --reason TEXT` also recognizes the former
`~/.stado/work/platform-matrix-cargo-target` cache. It refuses linked,
unrecognizable, younger-than-one-hour, or live-held trees; unrelated untagged
directories remain outside that cleanup. `host build-caches` reports
`no-cache-tags`, `root-protected`, or `scan-failed` rather than making those
cases look like a successful empty scan.

## Evidence

The real build journey lives in `stado-rs/tests/builds/`. It writes a recipe through the compiled CLI, lets the coordinator observe the public Stado repository, lets a real platform-matching worker claim the job, downloads `build-output.txt`, and verifies its bytes. Probierz registers this as the `native-build` journey and retains the source-bound report.

The full release journey lives in `stado-rs/tests/ci-cd/`. It builds committed source on a real worker, reads the signing key through an isolated real Skarbiec grant, publishes and installs the candidate, and executes the installed binary. Probierz registers this separately as `release-pipeline`, because a native build is not release evidence.

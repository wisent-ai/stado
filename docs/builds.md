# Native builds

Stado stores native build recipes in the canonical registry. A recipe names a public HTTPS Git repository, a branch, one POSIX shell command, the artifact paths produced by that command, and one or more worker platforms. The coordinator watches the branch and enqueues one job per platform when its head changes. A worker can claim only the job for its own platform.

Builds and releases are separate. A build uploads the declared artifacts and may record the exact semantic-version tag found on the commit. It does not sign an artifact, change `release_control`, or promote a release. `stado release submit` owns qualification, signing through Skarbiec, publication, delivery, and installation.
When Stado itself is delivered, the installer records the installed version
beside the managed binary. Each managed queue agent finishes its active slots,
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

## Release publication authority

Release signing and authenticated release-object publication resolve one
declared credential authority: `release_api.skarbiec.url`. They use separate
least-privilege identities and owner-only token files on that same Skarbiec:
`stado-release-coordinator` reads only the signing key, while
`stado-release-api-verifier` reads the product publisher bearer named by
`release_api.publishers`. `STADO_API_TOKEN` remains a generic object API
credential and is never substituted for a product's release publisher.

Exact `GET /api/release/object?uri=stado://releases/...` reads and byte ranges
are public so a damaged credential plane cannot make signed recovery artifacts
unreachable. The public route rejects the generic object API's `versioned`
query mode rather than bypassing release-backend resolution. Release writes
and listings remain authenticated by the product publisher contract above.

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

Probierz owns the combined `platform-matrix` journey in `stado-rs/tests/platform-matrix/`. It runs macOS through the managed host channel and submits Linux to the pinned `local-ubuntu-server` worker through the normal Stado queue, so an inbound SSH port is not a requirement. The Linux worker verifies the digest of the published Skarbiec binary before using it and keeps a managed Cargo cache under `~/.stado/work`. The journey runs the platforms one after another because both release checks use the same canonical test product and version.

## Evidence

The real build journey lives in `stado-rs/tests/builds/`. It writes a recipe through the compiled CLI, lets the coordinator observe the public Stado repository, lets a real platform-matching worker claim the job, downloads `build-output.txt`, and verifies its bytes. Probierz registers this as the `native-build` journey and retains the source-bound report.

The full release journey lives in `stado-rs/tests/ci-cd/`. It builds committed source on a real worker, reads the signing key through an isolated real Skarbiec grant, publishes and installs the candidate, and executes the installed binary. Probierz registers this separately as `release-pipeline`, because a native build is not release evidence.

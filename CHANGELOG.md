# Changelog

## 0.16.26

- **Complete reader delivery:** root installation retains its verified archive and then updates every declared private Stado reader through the installed receiver. Partial retries leave identical root bytes unchanged but still check all global reader images; a failed root lifecycle is not repeated in the same apply. Cached launchd definitions are joined by native PID ownership, not by matching old arguments against a newly written plist.
- **Recovery ownership:** the resident worker verifies its acquired lock descriptor on macOS and preserves that lock during confined noninteractive privileged snapshot reads. Resume reuses a staged release's recorded origin, supports path-only object API declarations, distinguishes a completed PID-less launchd unit from a starting worker, and refuses conflicting actions against an active owner.
- **Native autostart state:** the read-back after enabling or disabling a launchd unit accepts both boolean overrides and native `enabled`/`disabled` names, preserving the captured boot state.
- **Authority mutation receipts:** service ensure waits for a real registry read after changing a unit, without repeating either the host action or the conditional registry write. A later recording failure states the already-completed host action and actual process id.
- **CLI and Desktop:** host-wide and selected-binary apply retain the complete CLI report and exit status, including failed private-reader JSON, stdout, and stderr. Both delivery adapters use the single CLI convergence path; the GitHub adapter retains its exact-source native driver between jobs.
- **Migration and rollback:** the retained-archive receiver contract first appears in this source, not the already-bound 0.16.24 or 0.16.25 sources. Deploy 0.16.26 before resuming private readers. Existing storage schemas are unchanged; rollback restores the prior reader and recovery defects.
- **Platforms:** native delivery covers `darwin-arm64` and `linux-amd64`; the lock and cached-definition corrections cover macOS. Real product-owned Probierz journeys cover both directions of a cached launchd program change.

## 0.16.25

- **Reconciliation retries:** before fencing writers, `host storage-root-reconcile` reads the current canonical Stado version instead of reusing the version in an older captured target. An existing fence keeps its staged version pinned; an unavailable registry is an error, not permission to use cached declarations.
- **Live transaction status:** owner reports retain `recorded_status` and include the native manager's current observation. A previously executing owner is reported as interrupted when its process is gone, or unobserved when the manager cannot be read.
- **Release qualification:** includes the formatter-required layout of the dashboard's object authorization calls and carries the proxy, storage, and archive changes below. The 0.16.24 publication stopped at `fmt`; its coordinate remains bound to `405fd806c9ac3884c24c73778813c6743e4e4e3e` rather than being overwritten with different source.

## 0.16.24

- **Stable proxy startup:** activation and rollback wait for the stable readiness endpoint using the product's declared readiness timeout. A live process or a fixed 200 ms delay no longer substitutes for a listening, forwarding proxy; an exited process still fails immediately, and an expired wait reports the last observed error.
- **Storage authority handoff:** `host storage-root-reconcile` now runs as a target-resident, globally locked transaction with durable run/resume/status/rollback/finalize receipts. It snapshots complete physical A and B roots, copies only the qualified `ecosystem/` namespace and matching metadata additively from B to A, captures and restores the exact native service, queue, autostart, routing, and mapped-executable state, activates the target's dynamically declared published Stado runtime, and leaves lifecycle cleanup to the ordinary coordinator before observation-only finalization.
- **Object API recovery:** loaded process identity, storage root, and authenticated reads determine readiness, rather than the plist alone. Recovery shares the storage-handoff lock and refuses to change authority; `host storage-root-reconcile` remains the single snapshot, copy, and rollback implementation.
- **Runtime storage identity:** `/api/state.json` reports the serving PID, version, backend, and local root. Direct-primary requirements and primary-only registry reads are preserved.
- **Service archives:** extraction is staged beside an installed version, the declared executable must exist before activation, existing immutable contents remain intact, and `current` changes atomically. Replaying the active archive preserves the existing no-relink behavior.

## 0.16.23

- **Release coordinate:** this coordinate remains bound to source `d38c960e82747fd94e954eeaba0fd202e5509a16`. The integrated authority, recovery, and archive changes use a new coordinate rather than overwriting that source identity.

## 0.16.22

- **Canonical job targets:** direct submissions and schedules resolve `--pinned-host` from the configured authoritative registry, not the registry bundled into the executable. Newly declared targets therefore address their actual worker instead of remaining queued under an unresolved target name; an unreadable registry is reported rather than silently selecting stale metadata.
- **Native stream reconciliation:** stream setup records the complete Xorg and Sunshine service definitions, including dependency ordering and startup conditions. Generic repairs retain those definitions and declared environment; an explicit program override changes only the start command. Setup replaces changed files atomically, preserves healthy unchanged services and existing credentials, and refuses success unless both services are active at the declared display size. Initial credentials use Sunshine's loopback API without placing secrets in process arguments.
- **Authoritative host health:** beacon reads stay on the primary and select the newest registry-owned alias. A timer-triggered oneshot reports scheduled health only with its native trigger and execution evidence, rather than masquerading as a continuously running process.
- **Publisher recovery:** a publisher can reconcile its service without rewriting repository secrets. A registered publisher reported online by GitHub is preserved; an offline one is repaired without replacing its registration.
- **Interrupted release recovery:** after a worker disappears, the coordinator can complete its expired job from the already retained request, passing receipt, and byte-verified archive instead of rebuilding it. A broken schedule is reported without preventing queue recovery; live leases and fresh checkpoints still prevent reclamation.
- **Migration and rollback:** deploy this version before storing authored `systemd_unit` definitions. Older reconcilers do not retain that field and can erase specialized native startup semantics; rollback requires removing those authored services from automatic generic reconciliation first.
- **Platforms:** native streaming definitions apply to Linux; canonical submission, host observations, and publisher reconciliation cover both supported native platforms.
- **Darwin process identity:** launchd label inspection fstats the already-open executable descriptor before and after hashing it, so the release gate compares the mapped device, inode, and SHA-256 without substituting metadata for the descriptor pathname.
- **Queue-agent lifecycle:** host delivery reuses the installed-release-handshake classification and leaves an active queue agent to recycle after its current slot instead of restarting it mid-job.
- **Failure evidence:** host delivery retains the complete pre-activation unit identities and the post-restart PID, start time, executable, device, inode, SHA-256, and explicit identity-unavailable reason when image proof fails.

## 0.16.21

- **Run-manifest retirement:** the v2 migration reports an exact missing manifest as typed storage `NotFound`; `read_run` translates only that error for the same bound path to absence and preserves every other storage or validation failure.
- **Delivery errors:** failed structured host-release reports retain their `error` detail in `service converge` receipts instead of collapsing every cause to the status word `failed`.

## 0.16.20

- **Terminal cleanup:** the coordinator treats run-reaper failure as degraded maintenance and continues scheduling; terminal cleanup reads the versioned manifest directly and stops that run without deleting job blobs when its deletion fence has disappeared.

## 0.16.19

- **Linux service creation:** `service deploy` and the first-install path of `service ensure` expand the same host-home and account placeholders as existing-unit reconciliation. A newly installed resolver no longer receives literal template values as `HOME` and `STADO_CONFIG`.
- **Release capacity contract:** stable deployment reads `free_gb`, `low_watermark_gb`, and `target_free_gb` from the `disk` object emitted by `host gates --json`. Candidate admission and post-cleanup retention therefore judge the authoritative report instead of treating nested fields as absent.
- **Migration and rollback:** existing units remain eligible for normal definition convergence, with no registry schema changes. Rolling back restores unexpanded placeholders on new Linux units and the stale top-level disk lookup that prevents release publication before platform object mutation.
- **Platforms:** corrected release admission applies to `darwin-arm64` and `linux-amd64`; Linux unit creation uses the native systemd owner, and Darwin service rendering is unchanged.

## 0.16.18

- **Measured CPU admission:** available cores use operating-system processor-time deltas instead of runnable-process load averages, so a high load average cannot falsely close a host whose CPU is idle. Unavailable measurements remain explicit, and already-owned jobs still reserve their requested cores.
- **Linux release readers:** a system-scoped queue worker now addresses its owner's user systemd manager with the same runtime directory and bus address as service operations. Release installation can refresh the running user resolver instead of failing to enumerate it after replacing the binary.
- **Failure reporting:** failed systemd reader operations retain the command's exit status and stderr; delivery still fails unless each replaced reader maps the installed executable.
- **Canonical archive layout:** both native recipes now stage `stado` at the archive root, matching the existing release workflow and declared `current/darwin-arm/stado` readers. The candidate bootstrap and all three host installers consume that same member; private services no longer need a differently packed archive.
- **Existing native journeys:** build qualification checks the completed artifact without requiring a fleeting queued state. Stale-lock qualification retains its filesystem and persisted-outcome checks without assuming a policy mode before policy resolution; the integrated isolated registry fixture names its actual native platform.
- **Migration and rollback:** no stored data changes. Rolling back restores inherited-login dependence during Linux delivery and the incompatible `bin/stado` layout of the earlier native pipeline; published coordinates are not rewritten.
- **Platforms:** the existing signed `darwin-arm64` and `linux-amd64` build and delivery paths remain unchanged.
## 0.16.17

- **Complete native delivery:** `release install-local` updates every registry-declared service-local Stado executable from the same verified archive and requires its existing image-convergence check to succeed. Archive validation reads the unit's actual executable, preserving quoted paths and launchd `Program` precedence instead of parsing presentation text.
- **Fleet delivery completion:** the stable deployment iterates each discovered service-local Stado reader before invoking its archive update. An empty reader set still succeeds, while every declared reader receives the canonical platform archive and image refresh instead of the shell aborting on an unbound loop variable.
- **Safe disk recovery:** tagged-cache pruning checks activity in the owning project, including Cargo's working directory. Host reclamation includes the current persistent job directory and keeps queue-active or process-held work.
- **Systemd environment migration:** `service env-unset` removes assignments from the same declared unit/drop-in paths supported by `env-set`, removes an emptied drop-in, and reloads definitions without restarting the service.
- **Migration and rollback:** no stored schema changes. Rolling back loses complete native and fleet private-reader delivery, persistent-work reclamation, and systemd assignment removal; published platform artifacts remain available for a corrected delivery.
- **Platforms:** both delivery paths use the existing signed `darwin-arm64` and `linux-amd64` releases.
## 0.16.16

- **Native disk recovery:** queue cleanup resolves the operating system's legacy temporary root before opening its directory descriptor. The normal macOS `/tmp` symlink no longer aborts the candidate lookup and disables cleanup of terminal jobs in the persistent work directory.
- **Deletion safety:** job entries still use non-following descriptor-relative operations, ownership and device checks, bounded passes, and the authoritative live-job keep list.
- **Registry observations:** `registry doctor` derives its typed and raw views from one authoritative registry generation. Canonical reads no longer silently use a disaster-recovery replica or a product namespace, and live beacon/capacity reads stay on the primary.
- **Linux service environment:** the systemd environment writer executes its Python input as a script rather than treating the service UID as a filename. A refused write retains the writer's actual error; a corrected unit definition is reloaded without restarting its service.
- **Migration and rollback:** no stored data changes. Rolling back restores the unresolved legacy-root open, stale registry observations, and the broken systemd environment-writer invocation.
- **Platforms:** `darwin-arm64` and `linux-amd64` remain supported through the existing signed release and delivery paths; the native Probierz matrix retains exact-source qualification.

## 0.16.15

- **Coordinator archive delivery:** `service update --from-archive` validates members against the fixed `darwin-arm/` directory the installer actually supplies, so canonical root `stado` satisfies a unit running `current/darwin-arm/stado`.
- **Complete reader delivery:** after both platform publications, the stable fleet job walks every manifest-required target and retains ordinary `host release` convergence for each target's host-global Stado image. Native install identifies long-running direct or launcher-owned readers by kernel device and inode, restarts each replaced image through the unit's declared lifecycle, and fails unless its replacement process maps the installed inode. The fleet job also discovers every registry-declared service-local Stado reader, installs the target platform's canonical archive into each private version tree, and uses that same kernel identity to restart only readers not already on the delivered inode.
- **Resumability:** after delivery, `service converge --apply` runs the kernel-backed reader pass only when the Stado root is freshly observed both byte-attested and in sync. A prior files-only delivery can therefore finish its runtime half without allowing host-ahead, unknown, unattested, or failed pre-install state to launch an old or untrusted mutator.
- **Convergence verdict:** unread image identity or lifecycle ownership, a failed restart, a replacement process that does not map the installed inode, or a failed release receipt remains a failed delivery even if a later installed-version read says the path is in sync.
- **Recovery:** these changes let the active coordinator consume the canonical Stado archive and resume its already-committed release-control handoff without manually replacing files, repacking an archive, or reapplying the CAS.
- **Migration and rollback:** no stored-state migration is required. Rolling back restores delivery that can stop after replacing only the root pathname while declared readers retain older mapped images.
- **Platforms:** the native delivery and reader-verification path covers `darwin-arm64` and `linux-amd64`.

## 0.16.14

- **Canonical queue reachability:** the queue's HTTP client now uses the same Tailscale name-to-address map as artifact delivery. A broken system MagicDNS resolver no longer prevents submission, status, or cleanup from reaching the configured queue.
- **Connection reuse:** pooled clients are keyed by origin host and CA configuration, so a connection pool cannot carry another origin's address pin. Bearers remain per-request headers; TLS hostname and certificate verification are unchanged.
- **Migration and rollback:** no configuration or stored data changes. Rolling back restores dependence on the system resolver for queue operations.
- **Platforms and evidence:** the native Probierz qualification covers `darwin-arm64` and `linux-amd64` and records the exact source revision.

## 0.16.13

- **Coordinator archive delivery:** `service update --from-archive` validates members against the fixed `darwin-arm/` directory the installer actually supplies, so canonical root `stado` satisfies a unit running `current/darwin-arm/stado`.
- **Complete reader delivery:** after both platform publications, the stable fleet job walks every manifest-required target and retains ordinary `host release` convergence for each target's host-global Stado image. Native install identifies long-running direct or launcher-owned readers by kernel device and inode, restarts each replaced image, and fails unless its replacement process maps the installed inode. The fleet job also discovers every registry-declared service-local Stado reader, installs the target platform's canonical archive into each private version tree, and uses that same kernel identity to restart only readers not already on the delivered inode.
- **Resumability:** `service converge --apply` runs the kernel-backed reader pass even when the installed file is already attested at the declared version, so a prior files-only delivery cannot become a false successful retry.
- **Convergence verdict:** unread image identity, a failed restart, a replacement process that does not map the installed inode, or a failed release receipt remains a failed delivery even if a later installed-version read says the path is in sync.
- **Recovery:** these changes let the active coordinator consume the canonical Stado archive and resume its already-committed release-control handoff without manually replacing files, repacking an archive, or reapplying the CAS.
- **Migration and rollback:** no stored-state migration is required. Rolling back restores delivery that can stop after replacing only the root pathname while declared readers retain older mapped images.
- **Platforms:** the native delivery and reader-verification path covers `darwin-arm64` and `linux-amd64`.

## 0.16.12

- **Release staging:** both native platform recipes name the Stado executable relative to the worker's output directory, without duplicating `.wisent-output`.
- **Software inventory:** unsupported executables are not launched to discover their version. Catalogued version commands have a five-second deadline, helper classification overlaps bounded reads, and an unknown version no longer prevents other programs from being reported.
- **Version declarations:** `host declare-version --unset` removes one obsolete product requirement with a generation-checked registry write; repeating it reports that the declaration is already absent.
- **Cache cleanup failures:** a failed tagged-cache removal returns a nonzero exit status and retains the filesystem's stderr instead of reporting a successful command.
- **Migration and rollback:** no stored-state migration is required. Rolling back restores the duplicated staging path, unbounded version discovery, and the inability to remove one version declaration through the CLI.
- **Platforms:** the existing `darwin-arm64` and `linux-amd64` release and delivery paths are unchanged.

## 0.16.11

- **Native qualification footprint:** the macOS and Linux release journeys omit debug symbols and incremental compiler caches from their disposable test builds. Runtime checks and the host's disk admission thresholds are unchanged.
- **Terminal cleanup:** the Linux matrix removes its own Cargo output on both success and failure, and keeps the downloaded signing tool in its ignored work directory instead of making the source checkout dirty.
- **Migration and rollback:** no stored data changes. Rolling back restores the larger temporary build footprint and leaves Linux qualification caches until normal retention removes them.
- **Platforms and evidence:** the existing Probierz matrix still requires native build-artifact delivery, signed installation, and cancelled-build retry on `darwin-arm64` and `linux-amd64`.

## 0.16.10

- **Release-capacity correction:** local `host reclaim` now runs disk cleanup exclusively through the exact Stado binary that owns the invocation. A tagged release can therefore use its corrected bounded janitor during pre-publication capacity recovery instead of re-entering the older installed janitor that is holding the retired lock; remote targets retain their installed authoritative executable.
- **Lifecycle boundary:** no manual service restart or bootstrap is added. Normal digest-pinned `install-local` delivery retains the existing idle-slot agent handoff and launchd service declaration.
- **Migration and rollback:** no stored data changes. The immutable 0.16.9 tag remains at its original source and published no release objects; rolling back restores installed-binary-first pre-publication cleanup.
- **Platforms and evidence:** the local release-capacity path runs on the native control-plane platform; the existing remote reclaim path and installed-candidate fallback are unchanged.

## 0.16.9

- **Product delivery:** non-Stado releases now run the installed Stado delivery worker against the signed product archive instead of requiring that unrelated archive to contain `bin/stado`. Stado self-delivery still runs the digest-pinned candidate worker so it can repair an older installed worker.
- **Host disk diagnostics:** `stado host disk` now attributes Linux pressure inside the managed home, `/home`, `/mnt`, `/var`, and `/opt` instead of returning an empty inventory after a depth-two root report only named its parent directories.
- **Build cache recovery:** platform-matrix Cargo output now belongs to its queue workdir and follows terminal-job cleanup. `host reclaim` can remove the former exact managed cache only after checking its Cargo identity, age, symlink boundaries, and absence of live users; `host build-caches` reports missing tags and scan failures instead of hiding them as an empty result.
- **Migration:** no configuration or persisted-state migration is required.
- **Remote profile migration:** `stado host config-set` migrates an older deployment profile through the installed binary before applying the field, preserving the exact prior profile without requiring a separate operator step.
- **Configuration reload:** `host config-set --reload-service` and `host config-unset --reload-service` restart the existing managed unit in place. Adopted units no longer need a separate service build recipe just to read their changed configuration.
- **Rollback boundary:** rolling back makes every non-Stado delivery fail before installation with exit 126 because the product archive does not contain a Stado executable.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; the cancelled-build retry journey builds, publishes, delivers, installs, and executes a real non-Stado product through the repaired path.
- **Platform regression evidence:** the existing fleet journey now uses current durable job identities, retains its complete remote job reports, and exercises the cancelled-build retry on both native platforms.
- **Bounded cleanup:** build-cache directory enumeration now checks the pass deadline while reading names instead of draining an arbitrarily large directory before the first budget check. A partial enumeration is reported as an incomplete pass and authorizes no deletion.
- **Lock recovery:** taking over an overdue cleanup lock, or finding a still-held retired lock inode, persists the existing `lock_recovery_report_only` outcome and returns immediately. Recovery no longer repeats the unbounded filesystem scan that stranded the predecessor.
- **Owned command cleanup:** every command launched through Stado's shared production runner owns a fresh local process group containing its shell or SSH client and local descendants. Timeout or cancellation kills that locally owned group before releasing supervision, so a timed-out local diagnostic cannot orphan its own `du` child. The boundary does not claim that processes beyond an SSH connection joined the local group.
- **Migration and rollback:** no stored data changes. Existing overdue cleanup owners must be closed by exact PID/unit identity through Stado before retrying cleanup. Rolling back restores unbounded directory enumeration, recovery rescans, and direct-child-only timeout cleanup.
- **Platforms and evidence:** the affected cleanup and command-runner paths cover both native platforms. Formatting, locked compilation, and Clippy cover the corrected source; immutable publication retains source and platform identities.

## 0.16.8

- **Guarded handoff delivery:** carries the complete interruption-safe release-control handoff from 0.16.5 together with the newer installed fleet functionality already merged through 0.16.7. Matching prepared receipts can refresh a stale expected generation only after the same intent, exact runtime files, and still-managed lifecycle are re-proved; a registry-committed resume skips the successful CAS, reacquires the service lease, verifies the exact active release, and finishes the reconciler fence.
- **Release correction:** the immutable 0.16.5 tag remains attached to its original source, whose declared Clippy gate rejected an eight-argument internal helper before publication. This coordinate groups the three legacy identities into one fixed array without changing the checks, uses the first unoccupied version after the already-owned 0.16.6 and 0.16.7 coordinates, and does not move or overwrite either tag.
- **Migration and rollback:** no stored data changes. Install a compatible release on every registry reader before handoff; rolling back below 0.15.24 makes the external placement shape unreadable, while rolling back below this release removes interruption-safe completion.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; formatting, locked compilation, and Clippy cover the corrected source, while immutable release publication supplies signed manifests, artifacts, source identity, and delivery receipts.

## 0.16.7

- **Exact storage-root evidence:** `stado host backup-audit TARGET --object STADO_URI` compares only the named existing object in the host's declared local primary and backup roots. The read-only report returns each side's state, byte count, and SHA-256 without returning object content or walking either store; repeat `--object` for additional coordinates.
- **Safety boundary:** exact-object mode accepts validated `stado://` object references, conflicts with replica reclamation, and stops hashing at the existing read-only deadline with an explicit `deadline_unproven` state instead of allowing the host channel to truncate. The existing whole-replica classification and same-pass `--reclaim-twins --apply` behavior are unchanged.
- **Migration and rollback:** no stored data changes. Rolling back removes exact root comparison and leaves operators unable to distinguish an authority switch from lifecycle deletion without a broad replica scan.
- **Platforms and evidence:** the diagnostic uses the existing host channel on the supported native platforms. Formatting, locked all-target compilation, and Clippy cover the source change; release publication supplies native manifests and source provenance.

## 0.16.6

- **Terminal workdir retention:** disk cleanup keeps its listing-only live-job scan, then reads lifecycle bodies only for bounded on-disk candidates whose names overlap live and terminal prefixes. Only a typed retired transition, a matching terminal destination, and no live, fenced, malformed, or unknown document authorize removal. Cleaned sources must retain the expected immutable submission identity; the current source must name the current transition, while valid historical cleaned transitions in other prefixes remain supported.
- **Storage authority:** destructive janitor queue and registry reads stay on the configured primary, including through the Stado object adapter. The object API server refuses a self-referential Stado primary. Both paths use the existing replica wrapper in primary-read mode, preserving compatible backup writes; ordinary clients retain read failover.
- **Migration:** no queue or registry rewrite is required. Existing valid cleaned transition sentinels remain durable, and no global job-document scan is introduced.
- **Rollback boundary:** rolling back restores permanent retention of workdirs whose cleaned queue/running sentinels outlive run reaping, lets Stado-configured destructive readers treat a separately configured backup as authority, and lets other compatible-backend authority reads fail over after primary errors.
- **Platforms and evidence:** the affected cleanup and object-server paths support the existing native platforms. Formatting, locked all-target compilation, and Clippy cover the source change; release delivery retains platform manifests and source provenance.

## 0.16.5

- **Atomic handoff fencing:** `service handoff-release-control` now refuses a placement profile already owned by an active move transaction before it contacts the host. Runtime checks and the sole registry compare-and-swap run under the existing per-unit service lifecycle lease. Before that CAS, the command durably writes the exact cleanup identities and intended handoff as a `prepared` product work receipt; after it, the receipt advances immediately to `registry_committed`, then records a post-CAS reconciler-report baseline, waits for a newer report, and rechecks both the unloaded legacy label and absence of old-executable callers. Reinvocation validates a matching receipt, exact release, and targeted registry state, skips an already successful CAS, reacquires the same lease, records the observed recovery generation without inventing a lost CAS generation, and finishes the shared fence path. A still-managed retry whose CAS lost to an unrelated registry write refreshes only the prepared generation after repeating the exact checks and preserving its original cleanup tokens.
- **Migration:** no persisted-state rewrite is required. Install this release on every registry reader before performing a release-control handoff.
- **Rollback boundary:** rolling back restores the race in which an already-claimed placement move or stale generic service reconciliation can act on the legacy lifecycle while handoff removes it, and removes the write-ahead cleanup receipt, interruption-safe resume path, and post-CAS reconciler-report fence.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; locked compilation covers the source change, while release publication supplies signed platform manifests and delivery receipts.

## 0.16.4

- **Object API recovery:** an absent launchd job is bootstrapped even when its plist already matches the intended definition. A changed definition is persisted before unloading, so an interrupted recovery does not lose the file needed by its next invocation.
- **Launchd definition convergence:** `service ensure` now compares the desired Program and argument vector with launchd's retained definition, not only the plist. A stale retained definition is reloaded only after executable and plist preflight, and success requires launchd readback plus the running executable to match. Rollback is attempted only when the prior on-disk definition genuinely differs; an already-desired plist is not reactivated after the replacement lifecycle fails.
- **Recovery deadline:** the protected-read wait now uses 180 elapsed seconds rather than 180 potentially slow network attempts.
- **Executable ownership:** the object API now runs the host's canonical delivered `$HOME/.stado/bin/stado`, whose version is governed by `targets[].managed_versions.stado`, instead of an independently content-addressed service image whose source checksum was not retained in the service declaration. The catalog, physical-store recovery definition, registry reconciliation, and Stado product unit ownership agree on that one path; release activation restarts the object unit only on a host whose registry service set declares its exact label.
- **Migration:** run the existing `stado host recover-object-api TARGET` to repoint and recover the physical unit, then reconcile the managed service declaration with `stado service ensure --from $HOME/.stado/bin/stado`, the existing dashboard argv, and the unit's explicit recovered environment. Recovery alone does not rewrite the registry.
- **Rollback boundary:** rolling back can leave recovery trying to kickstart an unloaded job, waiting beyond the host-channel deadline, or pointing the object service back at an independently owned content-addressed image that the service declaration cannot restore.
- **Platforms and evidence:** the affected recovery command targets macOS; compilation and shell syntax checks cover the change, while its retained recovery report records the production outcome.

## 0.16.3

- **Release scheduling:** a live builder temporarily occupied by CPU, RAM, or an exclusive job can receive a queued release build when no builder is immediately available. Worker claim-time resource checks remain unchanged; disk, paused-queue, missing-measurement, and unexplained refusals still prevent selection.
- **Launchd diagnostics:** `service label-print` now reports the unit's declared stdout/stderr paths and at most twelve launchd events from the preceding hour whose delimited launchd identity field names the exact validated label. A broad text predicate only bounds the source scan; it never proves attribution. Event-read failure is explicit rather than indistinguishable from an empty result; the read-only command still omits the unit environment and never reads the declared log files.
- **Migration:** no configuration or persisted-state rewrite is required. Existing release submissions can resume with the same source and version.
- **Rollback boundary:** rolling back restores immediate submission failure when every matching builder is temporarily busy and removes the bounded launchd spawn/exit evidence from `service label-print`.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; compilation covers this change, and release records retain the selected builder and its eventual build outcome.

## 0.16.2

- **Release verifier:** `stado host reconcile-release-verifier TARGET` now compares the caller's publisher declarations with the target's effective configuration and binds the existing verifier bearer to the complete exact item set. Retired publishers no longer leave stale capabilities that close the release publication boundary.
- **Migration:** run the reconciler once on the release-object host. The command preserves the bearer and expiry, copies current publisher shadows, and removes only capabilities absent from the exact shared declaration.
- **Rollback boundary:** rolling back restores additive, product-at-a-time reconciliation, so a retired publisher capability can close publication again.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; compilation and the live exact verifier report cover this repair, while release publication supplies signed platform manifests and delivery receipts.

## 0.16.1

- **Workload liveness:** local-agent job shells now give Git HTTPS transfers a two-minute low-speed deadline and disable terminal credential prompts, so a connected but non-progressing fetch cannot hold a heartbeat lease and disk-cleanup lock forever.
- **Resource fidelity:** Cargo builds inherit `CARGO_BUILD_JOBS` from the CPU cores Stado reserved for that job. An undeclared job therefore keeps Cargo to its one-core fallback instead of each of ten admitted jobs fanning out across the whole host.
- **Reclaim availability:** when the configured queue primary is the local Stado object API, disk cleanup reads its live-job safety fence from the explicitly declared direct server backing store. Exhausting the host disk can no longer make an unavailable listener disable the canonical workdir janitor needed to recover that disk.
- **Release catalog:** synchronization stops at each checkout's manifest instead of importing build and dependency copies. Publisher commands read their bearer with the same configured consumer grant they acquire, not with the server's separate verifier identity.
- **Migration:** no persisted-state rewrite is required. Existing jobs receive the bounds when they next start under the updated local agent.
- **Rollback boundary:** rolling back restores unbounded Git HTTPS progress waits and lets each Cargo process ignore its Stado CPU reservation.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; the repository release pipeline supplies its standard qualification evidence before publication.

## 0.15.26

- **Release-store retention:** the host janitor now reclaims immutable product versions only after preserving every host-owned, active-pipeline, recently completed, quarantined, and newest rollback coordinate. It walks the canonical product/run layout without following links, enforces the declared byte and item limits, and reports every reason a version stayed.
- **Object API recovery:** the checked recovery path now accepts a healthy Skarbiec proxy that release-control still owns instead of mistaking recorded ownership for an orphan and refusing before it checks readiness.
- **GPU admission:** inference deployment now distinguishes GPU processes owned by registry-declared units from unknown processes. Declared streaming and service workloads may coexist with inference; an unaccounted process still refuses the deployment with its PID.
- **Migration:** enable `release_store` in the object-store host's disk-cleanup policy; the default rollback ladder keeps three newest versions per product. No persisted state rewrite is required.
- **Rollback boundary:** rolling back disables release-store reclamation, restores the false Skarbiec recovery refusal, and treats every non-inference GPU process as unmanaged.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; compilation and the live janitor report cover the new retention path, while the release pipeline supplies signed platform manifests and delivery receipts.

## 0.15.24

- **Release-controlled placement:** a placement service may now declare the exact external lifecycle `{"name":"<service>","controller":"release-control","product":"<product>"}` on every host template. Managed units retain their exact `name`/`unit`/`path`/`kind` record, routing units remain Stado-managed, and mixed or partial lifecycle records are rejected.
- **Atomic handoff:** `stado service handoff-release-control SERVICE --host HOST --product PRODUCT` proves the desired committed release, stable proxy, readiness, inactive legacy label, exact regular non-symlink legacy file identities, and absence of an executable caller before one generation-bound compare-and-swap removes the target service row and legacy restore fields while externalizing every host template. It emits unique, one-use retirement receipts containing each file's SHA-256, byte count, four-digit mode, and safe transaction token. Placement mutations then refuse the whole release-controlled profile before any action.
- **Safe residue retirement:** mutating `stado host retire-file` now requires all four binding fields from a handoff or reviewed dry-run receipt; it keeps its digest-, size-, mode-, and transaction-bound user-binary archive and also accepts one exact root-owned `/Library/LaunchDaemons/*.plist`, moved with approved sudo to a non-loadable sibling only while the receipt still matches.
- **Migration:** install 0.15.24 on every registry reader and agent before publishing the external lifecycle shape. Perform the handoff once, then consume its receipts directly—without another dry-run—to retire the legacy plist first and convenience binary second as separately reported operations; routes, endpoints, consumers, profile state, and probes stay unchanged.
- **Rollback boundary:** rollback before handoff is ordinary binary rollback. After handoff, an older reader cannot parse the external lifecycle and cannot safely own the service; restoring managed lifecycle requires a new generation-bound registry change plus restoring both archived files, and must not run alongside release-control.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; compilation, clippy, registry static validation, and shell static checks cover this release, while publication supplies its signed two-platform manifests and exact fleet delivery receipts.

## 0.15.23

- **Run retention:** validation of an already-retained terminal outcome now uses the same legacy-linkage rule as the reaper that records it. A terminal job may omit all three submission-linkage fields only when its job id and remaining immutable projection exactly match the durable manifest entry; live and partially linked jobs still require exact submission identity.
- **Migration:** no persisted-state rewrite is required. The coordinator can validate and complete the normal reaper repair for affected v3 run manifests.
- **Rollback boundary:** rolling back lets the reaper write the migrated outcome but makes the next manifest validation reject that same outcome as different submission content.
- **GUI host recovery:** interrupted CuaDriver downloads keep their version-scoped partial archive and resume from the last byte instead of restarting at byte zero until the host-channel deadline fails again.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; the run-retention journey drives the built coordinator through the migrated terminal state.

## 0.15.22

- **Desktop verification host:** managed GUI hosts now use the pinned and checksummed CuaDriver 0.23.2 bundle. `stado host gui-automation enable TARGET` replaces the previous managed driver and recreates its Aqua LaunchAgent when the installed version differs.
- **Test bundle:** `desktop/StadoDesktop/scripts/build-app.sh --unsigned-bundle` builds a complete bundle for transfer to a dedicated Probierz host without reading signing identities, installing the app, registering it with LaunchServices, launching it, or changing the local running app.
- **Migration:** no persisted-state rewrite is required. Reconcile each declared GUI host with `stado host gui-automation enable TARGET`; the existing Accessibility grant is re-established for the new signed bundle.
- **Rollback boundary:** rolling back pins CuaDriver 0.22.0 again and removes the side-effect-free test-bundle staging mode.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; the Desktop capacity journey runs on the Stado-selected macOS GUI host through Probierz.

## 0.15.21

- **Run retention:** the coordinator can retain an exact terminal job named by a validated durable run even when that historical job predates submission-linkage fields. Missing linkage is accepted only when all three linkage fields are absent and the remaining immutable job projection matches the manifest; partial or conflicting linkage still fails closed.
- **Migration:** no persisted-state rewrite is required. Existing affected manifests are repaired by the normal run reaper on its next successful coordinator tick.
- **Rollback boundary:** rolling back restores the coordinator failure `terminal entry without retained outcome` for a migrated run whose terminal job lacks submission linkage.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; the run-retention journey drives the built coordinator against the historical terminal state, and release CI supplies the remaining qualification evidence before publication.

## 0.15.19

- **Dynamic capacity:** hosts no longer declare a static job count. Admission now uses the CPU, memory, disk, accelerator, and active-workload measurements the host publishes, and the CLI, desktop fleet view, documentation, and behavior tests use the same model.
- **Service identity:** systemd unit names remain canonical instead of accumulating `.service` suffixes. `service label-print` now reads Linux units through the host channel, so migrations can prove the canonical unit is healthy before retiring duplicate legacy names.
- **Safe retirement:** `service retire` and `service remove` withdraw the declaration under the autonomy lease, wait for the active reconciler to observe that fence, disable and runtime-mask Linux units, verify they stay stopped, and delete their managed unit files without a coordinator reviving them mid-transaction.
- **Graphical services:** a graphical service remains a per-user LaunchAgent on an always-on macOS host; always-on placement no longer moves GUI-dependent programs into a LaunchDaemon session that has no graphical account.
- **Migration:** no persisted capacity conversion is required; obsolete fixed-count fields are ignored rather than treated as host limits. Duplicate managed unit names can be adopted and removed one at a time after their canonical replacement is observed running.
- **Rollback boundary:** rolling back restores static machine job limits, reopens the retirement race with the autonomy reconciler, and can recreate duplicate systemd names or place graphical services in the system launchd domain.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; release CI supplies the existing qualification evidence before publication.

## 0.15.18

- **Host probes:** `host exec` now approves the exact Appium, Android Debug Bridge, Git, tmux, Cua Driver bundle, and iOS Simulator spellings emitted by desktop-capture preflight while keeping each request normalized to the canonical read-only command.
- **Object verifier:** reconciliation first compares the target's complete object namespace declaration with the local declaration and refuses before changing a grant when either input is incomplete; it no longer reports an exact match from a partial view.
- **Job progress:** status for long-running jobs measures durable work progress instead of treating process liveness as completion evidence.
- **Release recovery:** resubmitting a release whose platform build failed creates a distinct deterministic retry with attempt-scoped outputs, while stable queue admission remains fenced by every durable lifecycle transition.
- **Migration:** no persisted-state migration is required.
- **Rollback boundary:** rolling back removes the complete-declaration guard and exact desktop probe approvals, and restores terminal platform-job reuse that can leave a release unable to recover.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; release CI supplies the qualification evidence before publication and required delivery receipts for all three fleet hosts.

## 0.15.13

- **Release identity:** `release active-binary` now binds the exact serving executable to the process group Stado recorded at spawn. This accepts the payload child supervised by macOS `sudo` without accepting an unrelated process that merely shares its version or port.
- **Release cleanup:** the leak sweep recognizes every process in a recorded release group as owned, so it no longer labels a healthy supervised payload as an untracked candidate.
- **Queue durability:** local job trees now live under the agent owner's `~/.stado/work/jobs` instead of `/tmp`, so external temporary-file cleanup cannot unlink a running workload's cwd and diagnostics. Release submissions carry a deterministic compatibility bridge that relocates an old agent's already-materialized job before work begins and leaves only the upload symlink that old agent needs.
- **Cleanup fencing:** `host reclaim` delegates queue-workdir removal to the canonical janitor instead of independently sweeping a stale queue snapshot; the janitor's exclusive lock remains the only path that can remove a terminal job tree, and a bounded slice of each scan is reserved for terminal old-agent links so a full live-job root cannot starve them.
- **Queue failure evidence:** an absent or non-directory persistent job tree now emits stable `workdir_missing` heartbeat and finalization diagnostics with the exact expected path, and the terminal error retains both that marker and the real workload exit code instead of misreporting a missing path as empty output.
- **Release retry durability:** every release worker uploads and read-backs its canonical and attempt log before exit; an owned failure receipt is also persisted, while successful workers require archive plus receipt and publish the attempt receipt last. Every response must report the local file's exact digest and byte count. Failure-evidence errors retain the worker's original nonzero exit, while a success-evidence error becomes failure. Random owner-only proof files live under validated work tmp and are removed after their proof line reaches the safe command log. Agent-reserved files open owner-only with `O_NONBLOCK|O_NOFOLLOW` before fstat, and admission plus janitor traversal opens every owned work-root component descriptor-relatively without following symlinks.
- **Skarbiec cutover:** the canonical policy declares `com.wisent.always-on.skarbiec` as the legacy owner of port 8895; every release reconciliation boots it out before the stable proxy binds.
- **Migration:** no persisted-state migration is required. Existing records already store the process-group leader pid.
- **Rollback boundary:** rolling back restores pid-only executable matching, which rejects every macOS release whose `sudo` supervisor and serving payload use separate pids, and it removes the declared legacy-port cutover.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; release CI supplies the existing qualification evidence before publication.

## 0.15.12

- **Release recovery:** repeating `stado release submit` for a run with a failed platform now creates one deterministic retry from the prior terminal job instead of returning that terminal job again. Each retry has an attempt-scoped output URI, so crash recovery reuses the same attempt without colliding with partial output from an earlier one.
- **Queue admission:** stable submission replay no longer recreates a job after any durable lifecycle transition has existed. First admission settles an active transition, rechecks every lifecycle prefix, and reports a durable-prior-admission conflict when transition history—including a retired record—already fences that job id.
- **Migration:** no configuration or persisted-state migration is required. Existing retired transition records become the durable admission fence they were intended to be.
- **Rollback boundary:** rolling back permits an interrupted stable submission whose lifecycle object is no longer visible to recreate the same job id in `queue/`, even though its retired transition proves a prior admission. It also makes a failed release platform reuse its terminal queue run forever, so that release cannot recover through another submission.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; release CI supplies the existing qualification evidence before publication. The release retry journey cancels the first real build, repeats the same release submission, and requires a distinct replacement job to publish and install the binary.

## 0.15.11

- **Affected CLI surface:** `stado release submit` now queues an exact host-pinned release delivery from that host's retained capacity publication even when the general scheduler has aged the publication out. Platform builders still require fresh, claimable capacity.
- **Agent handoff:** the managed agent now reads the semantic-version field from `stado --version` instead of mistaking the trailing source revision for the version. When the binary and running agent already match, it repairs a stale `stado.release-version` marker rather than entering a supervised crash loop.
- **Host recovery:** `stado host recover TARGET --release VERSION` now repairs the object API on the host declared by the service directory before recovering `TARGET`; it no longer assumes every recovery target runs its own object API binary. Signed recovery installation now uses the same approved local-or-SSH host channel as ordinary recovery, so the workstation can recover itself without an SSH listener. The checked recovery helper also accepts the current registry's `null` legacy launchd fields as an undeclared legacy unit.
- **Migration:** no configuration or persisted-state migration is required.
- **Rollback boundary:** rolling back restores the fresh-capacity prerequisite for deliveries and can deadlock a Stado repair when the target is busy or under disk pressure. It also restores the release-marker parsing defect, so a stale marker can stop a current agent from publishing capacity; no persisted state needs conversion.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`. The release-delivery recovery journey covers a stale target publication while the ordinary builder remains fresh, and the build-identity journey drives the real managed agent through stale-marker repair.

## 0.15.1

- **Affected CLI surface:** removed the standalone Cargo binary target `stado_fleet`. Use the existing `stado fleet` command family; it remains backed by the same fleet implementation. Added `stado host retire-file TARGET PATH --product PRODUCT`, with `--dry-run` and `--json`, for checked retirement of unmanaged executable residues on registered local or remote hosts. Stado Desktop uses the same command and binds its reviewed apply to the dry-run transaction, destination, SHA-256, size, and mode.
- **Release recovery:** added `stado release redeliver PRODUCT RUN_ID DELIVERY --retry-token TOKEN` for re-running one delivery from the exact newest completed candidate without publishing a new candidate or changing channels. A typed, CAS-managed transaction resumes the same stable job across interruptions, records terminal evidence separately from the backwards-compatible release-run schema, and restores the run state on both success and failure.
- **Migration:** no configuration or persisted-state migration is required. Migrate any external invocation of `stado_fleet …` to `stado fleet …` before upgrading.
- **Rollback boundary:** the newest complete two-platform release evidenced by the live release audit is Stado 0.14.10 from source `42b8b2749251929ed2ec74ea2a08550a545a503a`; both required public `release.json` commit markers are present, while 0.14.11, 0.14.12, and 0.15.0 are absent. The product release archives stage only `stado`, so that published artifact does not restore the unmanaged standalone entrypoint. Rolling back this compatibility change requires reverting the source cutover and rebuilding the prior `stado_fleet` target. Bytes archived by `host retire-file` are preservation evidence only; Stado provides no restore command for them.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`. The remaining `stado` binary was compiled with the locked release dependency graph; release CI supplies the repository’s existing qualification evidence before publication.

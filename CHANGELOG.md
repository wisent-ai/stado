# Changelog

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

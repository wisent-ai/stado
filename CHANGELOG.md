# Changelog

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
- **Rollback boundary:** rolling back permits an interrupted stable submission whose lifecycle object is no longer visible to recreate the same job id in `queue/`, even though its retired transition proves a prior admission.
- **Platforms and evidence:** the supported native platforms remain `darwin-arm64` and `linux-amd64`; release CI supplies the existing qualification evidence before publication.

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

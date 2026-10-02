# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.20](changelog/0.22.18-0.22.20.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- The `--disk-cleanup` role of `com.wisent.stado` keeps running when one pass cannot read its policy. The pass reports `invalid_or_unavailable_policy` and no `check_interval_seconds` when the store answers 502 for a tick, and the watch ended the whole host process with "the cleanup report names no check_interval_seconds", taking the resolver, the worker and the API down with it every time the fleet store blinked; launchd respawned it ten seconds later and every `stado://` client on the host saw a refused connection in between. The watch now reads its cadence from the host's registry declaration (through the last-known-good copy) when a pass reports none, and ends only for a target that declares no `disk_cleanup`.
- A settled job whose run manifest this build cannot read — a submission a newer Stado recorded without `yield_grace_seconds`, which builds before 0.22.19 require — is left for recovery with `transition_run_manifest_unreadable`, and the rest of the lifecycle pass continues. Before, the coordinator tick ended with `invalid submission options: missing field yield_grace_seconds`, the host process ended with its coordinator, and on the host that serves the fleet's object API that was the whole fleet store going away every ten seconds.
- `stado build status` judges a platform whose job record the queue's run reaper already retired by the job's receipt, as `release submit` does. The reaper settles a completed job on its own cadence, and a status read after it found no terminal record, left the platform `submitted` and the build `waiting`, and `release submit --build` refused a build both of whose platforms had passed.
- The release boundary no longer requires a publisher for every name in a list inside the binary (`ACTIVE_RELEASE_PUBLISHERS` is gone). A name added to that list — `film`, `obraz` — closed the release publication boundary of every host whose `release_api.publishers` had not caught up, so `release submit`, `storage objects releases` and `release catalog audit` answered `503 object authorization unavailable` for every product before any release of the new one existed. The declared table is held to its own rules; a product's publisher is declared by `build submit`/`release submit` for a product the host lacks.
- The release agent's finishing pass, like `build status`, judges a build job whose record the run reaper retired by its receipt, so a run whose builds passed is published and delivered on the next tick instead of waiting for a record that no longer exists.
- A release run whose deliveries are still queued or running on their hosts stays `delivering`; the pass that finds a delivery ended judges it, and the release agent's next tick (or `stado release resume`) collects the rest. Before, the pass that had just queued the deliveries read them back at once, found them unclaimed, and recorded the run `failed` — on which the delivery worker, a moment later, refused its own job with "the run is Failed, not delivering", so a published release reached no host.

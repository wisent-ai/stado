# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.33](changelog/0.23.12-0.23.33.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A held tree is no longer read as free (fab304dc):** `space reclaim` and the tagged build-cache sweep asked `lsof +D` whether a process held a tree and trusted its exit status, but lsof answers 1 whenever any file it was asked about is not open — with `+D`, any file below the tree — so every non-empty tree read as unheld, including one a running process had as its working directory. Both programs now share one probe (`lsof_holds`) that decides by lsof's listing and keeps the path when lsof is missing or fails.
- **`space reclaim` takes a path on its second unheld look, never by age (fab304dc):** the one-day `MIN_AGE_DAYS`, one-hour `CLONE_MIN_AGE_MINUTES` and the 15-minute local terminality grace (`HEARTBEAT_STALE_MINUTES`, now gone from the config) are gone. Every stage asks one guard, `settled`: no live process names the path, `lsof` shows none holding it, and an earlier `--apply` found it the same way with the same modification time. The first apply records the path under `~/.stado/work/host-reclaim-local-evidence/settled/` and lists it in `refused` as `unheld on this look; the next apply takes it if no process holds it and it is unchanged`; unreadable ownership keeps it as `process ownership could not be read; retained`. The duplicate Cargo git-checkout loop is gone. Journey `tests/disk_cleanup/reclaim.rs`.

- **The disk janitor is judged by its own promise and its process, not by four missed minutes (fab304dc):** the rule's 60-second `CHECK_SECONDS` and `host gates`' `STALL_INTERVALS` (4) are gone. `stado disk-cleanup --watch` now takes `--interval-seconds N` and refuses to run without it (`--watch needs --interval-seconds: the period the watch reads the volume at`); `--interval-seconds` without `--watch` is refused too. `stado serve --disk-cleanup` runs the watch at the host's `--health-interval-seconds` and is refused without it, and a host install that folds a standalone watch unit into the host process without a health cadence is refused by name. Every periodic writer (the watch, the agent's per-poll pass) records under `promises.<writer>` in the state file when its next pass will have written it — its period plus the longest pass and the longest measured lateness on record — and its pid. `space report` shows the promises, whether each promising process is alive, and the live `stado` pids; `host gates` counts the janitor as running while any promise holds or its process lives, reports `disk_cleanup_stalled` when neither does, when no pass ever succeeded, or when the last pass ran and failed, and `disk_cleanup_lock_held` while it runs and its last pass was turned away. State timestamps in `space report` keep their microseconds. Journey `tests/disk_cleanup/promise.rs` (target `disk-cleanup`).

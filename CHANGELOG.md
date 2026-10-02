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

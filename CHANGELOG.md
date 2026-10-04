# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.18](changelog/0.23.12-0.23.18.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A fleet store that cannot be opened no longer ends the host's Stado process (3e193231):** `stado serve --health-interval-seconds` opened the fleet store during startup, so a store behind another host's object API answering `503 object authorization unavailable` ended the whole process — resolver, release proxy and worker with it — on every launchd restart (81 restarts on one laptop while the vault host's disk was full), and every local client of its resolver waited until it timed out. The host-health role now opens the store itself and, while it cannot, prints `the fleet store could not be opened, so this beacon is not published; the next tick opens it again: <cause>` and keeps running.

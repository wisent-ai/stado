# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.6](changelog/0.22.18-0.23.6.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- A disk-cleanup pass that reached its cap starts the next pass at once only when it freed space; otherwise the next pass waits the policy's `check_interval_seconds`. A host whose free space sat between the low watermark and the target, with nothing left to reclaim, ran a pass on every agent tick, each holding the cleanup lock exclusively, and its agent refused every job, release deliveries included, with `cleanup_in_progress`.

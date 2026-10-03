# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12](changelog/0.23.12.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado service reap` works on Linux hosts:** on a non-Darwin host the reap program printed an unsupported marker nobody read and exited 0, so a reap of a Linux host answered an empty table that looked like "no duplicates". It now builds the keep set from each declared systemd unit's `MainPID` (system and user manager) and keeps every process in a declared unit's cgroup, so a child a double fork left without a parent is still held by its unit. The help now says what `--apply` does: SIGKILL, then a wait for each process to exit, not SIGTERM.

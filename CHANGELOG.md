# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.25](changelog/0.23.12-0.23.25.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The janitor's build-cache walk no longer opens applications' containers (3a9eaacd):** `~/Library/Group Containers` and `~/Library/Containers` join the roots the walk refuses (and `stado space report` prunes). They are sandboxed applications' data, consent-gated by macOS and full of iCloud-backed content whose every open waits on the file provider: on the laptop one pass spent over an hour opening Final Cut's `com.apple.CloudContent`, holding the cleanup lock while the signed release delivery the disk-pressure rule admits waited (`disk_cleanup_admission: cleanup_in_progress`). No build tool writes a tagged cache there.

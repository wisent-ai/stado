# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.60](changelog/0.23.42-0.23.60.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The same installation asked twice waits instead of restarting:** `stado product install` and `update` stop an older installation of the same surface that has placed nothing yet, so a newer request replaces it. When two sessions asked for the same thing on one host — `stado product install stado --surface cli` and the same with `--json` — each stopped the other's build as soon as it started, neither ever placed a file, and the host kept crash-looping on the Stado it was trying to replace. A holder whose recorded command asks for the same installation (the same arguments after the program path, `--json` aside) is now waited for, printed as `waiting for <lock>: held by pid <pid> … (the same installation, already under way)`; a different request still supersedes it.

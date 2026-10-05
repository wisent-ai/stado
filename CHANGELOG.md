# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.24](changelog/0.23.12-0.23.24.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`credentials item upgrade` says how many items a former owner still controlled (57a7f19d):** Skarbiec 0.4.8's `upgrade` moves items a former vault owner still controls to the current owner, because nothing could write them after `rotate-owner`. The text report prints `<host>: items a former owner controlled: <n> would move to the owner` (`moved to the owner` with `--apply`), and `-` from a Skarbiec build without the step; `--json` carries it as `pass.control`.

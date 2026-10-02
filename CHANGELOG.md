# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.19](changelog/0.22.18-0.22.19.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado service release` takes `--readiness-url` and `--readiness-timeout-seconds` together (cli.md rule 14). The readiness window was 30 s whenever a URL was named without one; now naming either without the other is refused by the parser with exit 2. Pipeline promotion already reads the window its release policy declares.
- `stado host user create --registry-source` takes `remote`, `local` or `auto`; `gcs` is gone (cli.md rule 14). It never meant Google Cloud Storage: it read the canonical registry from whichever store `WC_STORAGE_BACKEND` selects, which `remote` now says.
- `stado azure` is now `stado cloud` (cli.md rule 14): `stado cloud login --provider azure …` and `stado cloud repair-rbac --provider azure …`, with the same flags as before. A call without `--provider` is refused by the parser with exit 2. The operator console allows the `cloud` family instead of `azure`, and `examples/providers/enable-azure.sh` ends with `stado cloud repair-rbac --provider azure` (it ended with a bare `stado azure`, which runs nothing).

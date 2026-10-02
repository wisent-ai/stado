# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

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

- `stado_database::connect` opens a fleet SQLite database. `stado database place` now writes `pooler_url` (`sqlite://<file>`) into a SQLite item beside its host and path, and the library reads it without a certificate, since a file has no server to verify. Before, a SQLite database Stado created had no field any consumer could read. A missing file is refused naming the file and that it opens only on the host that holds it. `<PRODUCT>_DATABASE_URL=sqlite://…` needs no `_CA_FILE`.

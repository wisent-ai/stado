# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.4](changelog/0.22.18-0.23.4.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- A `database_api` section (or `WC_DATABASE_API_DATABASES`) that does not parse is reported as a configuration failure (`error_code=config`) by `stado database list`, `resolve`, `destroy`, `supabase adopt`, `stado web declare --database` and `stado web deploy`. It was printed with the right sentence but reported as an unattributed failure of Stado (`error_code=unknown`).

# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Supabase database commands read the management token by role (35a1f379):** `create`, `adopt` and `destroy` asked for an item named `SUPABASE_ACCESS_TOKEN` through the role-selecting read, which answers nothing for an item that plays no role, so each ended `SUPABASE_ACCESS_TOKEN has no field value`. They now read field `value` of the item playing `supabase-management`, and a fleet without one is told the `item retag` and `grant role-read` that give it; a database item (`<name>-database`) is read as the declaration names it.

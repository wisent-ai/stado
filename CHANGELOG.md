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

- `stado dns undelegate <zone>` hands a zone `stado dns delegate` moved into Cloudflare back to the registrar's own nameservers (cli.md rule 2). It is refused, naming the records and changing nothing, while Cloudflare serves a record the registrar's host list lacks; after the switch it reads the registrar back and fails if the registrar still does not serve the zone. `--json` prints the zone, record count and nameservers before and after.
- `stado credentials bootstrap-weles` and `stado credentials adopt-weles-vault` are removed (cli.md rules 1 and 20). Both served one product: one rebuilt Weles's items from transcript history, the other merged a retired Weles-only vault. Recovering a single item is `stado credentials harvest --restore NAME`, and writing one is `stado credentials put`.

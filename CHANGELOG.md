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

- `stado host user delete USERNAME --target T` requires `--confirm USERNAME` (cli.md rule 16): the account, and its home directory unless `--keep-home`, cannot be restored, so a missing or different confirmation is refused with exit 2 before the host is contacted, naming what would be removed. `--json` prints the target, SSH target, username, status, OS and whether the home was kept (rule 13).
- `stado fleet key ls --json` prints each stored SSH host key's item, key type and fingerprint as JSON (cli.md rule 13). A key whose context cannot be read now fails the listing with `cannot read the context of credential item <item>: <error>` instead of printing it with two blank columns.
- `stado vast list`, `unlist`, `status` and `monitor` print one `key: value` line per field for a person and the same answer as JSON with `--json` (cli.md rule 13); before, they printed JSON only. Stado Desktop's Earning screen passes `--json`.
- Read commands print text for a person and the same answer as JSON with `--json` (cli.md rule 13): `stado cost report` and `cost estimate` (the JSON carries every completed job's row and each bucket), `stado resources operations list` and `show`, `stado optimize explain` and `optimize policy show`, and `stado-coverage list`, `verify` and `retry`. `stado cost allocation|forecast|anomalies|savings` without `--json` now print one `key: value` line per field instead of indented JSON.
- `stado schedule show ID` prints the schedule's fields as text; `--json` prints the full persisted record it printed before. The MCP tools `stado_schedule_show`, `stado_cost_report` and `stado_vast_status` pass `--json`.
- `stado serve --api-local-store PATH` serves the object API from the local root PATH while the process's other roles — worker, resolver, release agent — use the storage the host's config names. It is `--api-storage` for a local primary spelled as a path, because a unit argument cannot carry the JSON's quotes, and it is what an operator's machine declares: its worker publishes capacity to the fleet's store through the `stado` route, and its API serves its own store. With `WC_STORAGE_BACKEND=local` on the whole process, the worker published to a store "that does not answer for the fleet" and the host built nothing. `stado bootstrap --local` folds the option like every other serve option.

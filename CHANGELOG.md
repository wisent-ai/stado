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

- The `stado database` commands that change a declaration or the database service, and `stado database place`, print `key: value` lines without `--json`; before, those changes ignored `--json` and `place` printed JSON either way. `place --json` stays one line, which `database create` reads from a remote placement.
- `stado profiles [NAME]` takes `--json`: the list as `[{name, description}]` and one profile as its JSON; without it the list prints one line per profile and one profile prints `key: value` lines. The MCP tool `stado_profiles` passes `--json`.
- `stado billing watch` is one pass and takes no `--interval` or `--once`; run it from a schedule (`stado schedule create --cron … 'stado billing watch'`, or cron on a machine in another cloud). A pass that cannot read the previous snapshot or store the new one now fails with that error and sends no alert, where the loop printed a warning and carried on.
- `stado azure unusual-activity diagnose|open-ticket` print `key: value` lines, or JSON with `--json`. `open-ticket` no longer sleeps on Azure's `Retry-After` while a `202 Accepted` case is being created: it reads the ticket once, reports `outcome: accepted` with the `pending_operation` Azure named, and a ticket it cannot read carries the read error instead of an invented status.
- Every host channel reaches its target again. The channel's SSH key, `stado-ssh-<host>`, is a fleet key Stado mints under that name, and it was still selected as a role, so every command that goes to another host — `host ping`, `service ensure --host`, `credentials item retag`, `release catalog declare-publisher` — refused with `credential store has no SSH key item "stado-ssh-<host>"` while the vault held it. It is read as named, like the other fleet keys.

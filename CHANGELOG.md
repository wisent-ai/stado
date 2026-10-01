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

- `stado fleet join` and `stado fleet reject` take `--json`. `join` prints the request as `key: value` lines between its sentence and the `approve` command, where it printed a JSON document in the middle of text; with `--json` it prints `{recorded, request, approve_with}` alone, and the note that the registry is not readable goes to standard error. `reject --json` prints `{rejected}`.
- `stado fleet create|assign|unassign|delete` take `--json`: `{created, generation}`, `{target, fleet, generation}`, `{target, left, generation}` (`left` is the fleet the machine left, or null) and `{deleted, generation}`. Without it each prints the same sentence as before.

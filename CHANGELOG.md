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

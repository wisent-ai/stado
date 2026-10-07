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

- **`stado database adopt` off the owner adopts this machine's declarations (35a1f379):** 0.23.41 ran the whole command on the vault owner, which adopts the owner's own `database_api` declarations: where the owner declares none it refused `<name> is not declared`, and `--password-file` could not be used. Now only the item read happens there: the owner's own Skarbiec reads each item whole through the host channel, the rewrite goes back the same way, and the declarations and `--password-file` are this machine's. An item the owner cannot read is refused naming the item and the host.

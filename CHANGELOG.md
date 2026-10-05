# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.21](changelog/0.23.12-0.23.21.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A caller record this account cannot read no longer stops every release install (1f861070):** a `sudo stado` wrote its caller record under the account's `~/.stado/callers` owned by root with mode 600, and every later `stado release install-local` on that account (the vault host's stado 0.23.21 delivery among them) ended with nothing but `Permission denied (os error 13)`. The program-replacement check now names such a record (`caller record <path> cannot be read by this account (...); it was not written by this account's programs and is left out of the check`) and installs; a process whose effective user does not own HOME records nothing.

# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.43](changelog/0.23.42-0.23.43.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A Skarbiec refusal on the vault owner is stated as refused, not as an outage (7353f650):** `stado credentials token mint`, `grant` and every other command that runs the owner's Skarbiec reported any answer Skarbiec gave as `infra_down` (exit 69, "retry later"), so `grant issue requires --ttl-seconds` from an owner whose Skarbiec predates grants that live until revoked read as a network problem to wait out. The host channel reaching the host and Skarbiec answering is now `refused`, printed as `<host>: Skarbiec <command> refused: <Skarbiec's line>`; a request for a grant until revoked adds which Skarbiec executable on which host answered, the one to bring to the release that implements that lifetime. A host channel that does not reach the host is still `infra_down`.

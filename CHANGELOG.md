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

- `stado identity relay-apple-challenge` and `stado identity issue-apple-capabilities` are replaced by `stado identity relay-challenge --provider apple` and `stado identity issue-capabilities --provider apple` (cli.md rule 1). A provider without an adapter is refused by name with the list of those that have one. Weles's Apple account placement calls the new form (weles `src/auth/apple-account-placement.mjs`), so a Weles carrying that change needs a Stado that carries this one.
- `stado host render-spis-admission-trust` is replaced by `stado host render-public-document TARGET SOURCE` (cli.md rule 1): any checked-in renderer runs against TARGET's own live vault and its document is printed verbatim. Stado refuses output that is not one JSON document or that carries private key material; the document's shape is checked by its consumer, so Stado no longer holds one product's receipt-trust schema.

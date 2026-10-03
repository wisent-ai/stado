# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.2](changelog/0.22.18-0.23.2.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado credentials put` no longer accepts `--route`, `--consumer` or `--grant-file`, and a delegated `stado credentials get` again requires `--field`. In 0.23.2 `put` took those flags but still wrote to the owner vault as the store administrator, and a whole-item delegated read looked the name up as a role; Skarbiec has no consumer-scoped write of an operator item, so a write stays an owner act and `put`'s help now says so.
- `stado service ensure` registers a product's acquisition scopes before it starts the product's unit, when the catalog entry names the scope catalog the installed release carries (`acquisition_scopes`; Weles does). Before, Weles placed on a host whose vault had never seen its scopes crash-looped on Skarbiec 401s until someone ran `stado credentials acquisition-scopes sync` by hand. A release without the file, or a vault that refuses, fails the ensure before anything is retired or started.

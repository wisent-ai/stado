# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.9](changelog/0.22.18-0.23.9.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado product` identifies a checkout by the origin it declares (`remote.origin.url`), not by `git remote get-url origin`, which applies the host's `url.<base>.insteadOf` rewrites: on a host that rewrites GitHub to an SSH alias no checkout was recognised, and an installation that had just cloned the product refused its own clone with `does not identify <owner>/<name> through its GitHub origin`. That refusal now names the URL it read.
- `stado credentials item show --host <host> <item>` reports the item's context descriptors (`login_method`, `account_ref`, `provider`, …) as `context: <name>=<value>` lines and a `"context"` array in JSON; a nested entry is named, not shown. It reported fields and tags only, so a login row Weles refused with `declares unsupported login_method (absent)` could not be checked from outside the vault owner. A host whose Stado predates this answers `context: not reported`.

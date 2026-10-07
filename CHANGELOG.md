# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.40](changelog/0.23.12-0.23.40.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado credentials item restore --host H ITEM` undoes `item delete` (8fe82496):** delete said Skarbiec keeps the deletion restorable, but no Stado verb restored it, so a step that still needs a deleted item had no way back. Withdrawing a retired product's release publisher reads that product's bearer, so after deleting the item first, `release catalog withdraw-publisher` failed `item is in trash` with nothing to run. `restore` runs Skarbiec's own `restore` on the owner host and reports the item returned.

- **`service directory publish` reports `*.url` forward markers as fossils (0d6a46ce):** publish writes only `<service>.local`, so every `~/.stado/forwards/<name>.url` is a forward someone opened by hand on a port they picked; each is now listed as a fossil with its address and age, and `--prune` removes it with the undeclared `.local` markers.

# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.29](changelog/0.23.12-0.23.29.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Each host's resolver keeps its forward markers (c9d6ecb4):** `~/.stado/forwards/<service>.local` was written only by an operator running `stado service directory publish` on that machine, so the vault host had no `brama.local` while the directory declared Brama on it, and Weles exited at every start with `no brama address: … brama.local cannot be read`. `stado resolver serve` now writes every marker the directory declares for its host when it starts, before `resolver status` reports `serving`, and on every refresh, rewriting only a marker that is missing or holds another address (`stado resolver wrote forward marker <service> -> <url>`). A refresh that loads a newer directory logs `stado resolver loaded directory generation <N>` once the markers follow it. Undeclared markers stay a finding for `directory publish --prune`; nothing here deletes one.

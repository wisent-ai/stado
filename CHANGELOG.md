# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.38](changelog/0.23.12-0.23.38.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado release catalog pin-input --path` pins only the paths a build reads (3583622c):** repeatable; each path must exist at the commit and keeps its place under the mount, and the report carries the paths and the archive's size. Brama's echo-web input was 2.28 GB as a whole tree (tracked media) for a 51 KB crate, an archive the object API could store but not return in one read.
- **Disk cleanup never deletes cargo's git dependency cache (9b60a249):** `CARGO_HOME/git` carries cargo's `CACHEDIR.TAG` like `CARGO_HOME/registry`, but only the registry was reserved, so under disk pressure the cleaner removed the git clones while cargo wrote them and every build with a git dependency failed with `failed to create temporary file '~/.cargo/git/db/…': No such file or directory`.
- **Replace promotion delivers a placement-backed product service (91d8665b):** a route placed by a profile must leave `managed_service` absent, and promotion read only that field, so every brama release since 0.4.48 ended `release product service "brama" has no managed service` after publishing. It accepts the profile's unit for the target and hands `service release` the logical name.
- **`service release` reads a program pinned in the release agent's layout (184fd9cf):** a declaration of `services/<dir>/releases/<version>/<platform>/<member>` was read as one version segment, so the archive was asked for `<version>/<platform>/<member>` and refused (`archive does not carry the declared executable 0.4.48/darwin-arm64/bin/start-with-skarbiec`). Both layouts map to `current/darwin-arm/<member>` for the archive check and the unit's repoint.

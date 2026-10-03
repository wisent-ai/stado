# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.10](changelog/0.22.18-0.23.10.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- A `stado product` service operation forwarded to another host runs there with the host's toolchain on `PATH`, Cargo found the way `stado host build` finds it (`~/.cargo/bin`, `/Users/Shared/.cargo/bin`, the Homebrew and `/usr/local` prefixes). The host channel runs no login shell, so a forwarded source build found no `cargo`.
- A release run supersedes, and fences deliveries of, only runs it replaces: a lower version, or the same version submitted earlier. Ordered by submission time alone, a release of stado 0.23.8 submitted after 0.23.10 cancelled 0.23.10's builds, and once 0.23.8 published, every delivery of a newer version refused itself as stale. Versions are compared by the same SemVer order `stado release version-gate semver-at-least` uses.

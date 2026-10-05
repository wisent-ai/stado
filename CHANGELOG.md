# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.26](changelog/0.23.12-0.23.26.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The build-cache walk also refuses simulator device trees (3a9eaacd):** after 0.23.26 kept the walk out of applications' containers, the next pass on the same laptop spent its time opening `~/Library/Developer/CoreSimulator`, every simulated device's whole file system. It joins the refused roots (and `stado space report`'s prune list); no build tool writes a tagged cache there.
- **A build step that is a script finds the same toolchain the step itself would (e2a36aad):** the release worker looked a bare step program up in `~/.stado/bin`, `~/.local/bin`, `~/.cargo/bin`, `/opt/homebrew/bin` and `/usr/local/bin`, but handed the step only the LaunchAgent's minimal PATH, so Oko's `bash release/quality.sh` gate failed `exit 127` on the `cargo` it calls. A step's PATH is now the worker's own directory, then those directories, then the inherited PATH.
- **A release replaced after it published reads as published (ed421a73):** a run that had published every platform and was delivering was marked superseded "before publication" when a newer run was submitted, and `stado release status --json` gave it phase `failed`, so Oko counted no release of that source as published although its signed artefacts were in the store and on the hosts its deliveries reached. Such a run now records `superseded by release run <newer> after publication; <newer> delivers from here`, and the listing reports phase `published` for a superseded run every submitted platform of which is published.
- **Every Cargo build Stado runs compiles through Kache:** `stado product cargo build|check|test|run|stage`, owner-local installs of Cargo recipes, every step of a release builder job over a Cargo source, and `stado host build` set Cargo's `RUSTC_WRAPPER` to the Kache compiler cache, at the version `stado-rs/data/work/compiler-cache.json` declares. A host without that version installs it with `cargo install --locked` before the first compile, and a build whose cache cannot be installed fails naming that step. Each build compiled every dependency again in its own target directory; a crate a host compiled once is now restored. `stado product compiler-cache status|ensure|remove` reports, installs and removes it. No Cargo configuration of the account changes.

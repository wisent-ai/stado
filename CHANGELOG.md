# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.55](changelog/0.23.42-0.23.55.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Apple's intermediate lives in one keychain that stays:** each signing scope imported Apple's WWDR G3 intermediate into its own temporary keychain and deleted it with the scope. On charless-mac-mini `codesign` kept failing `unable to build chain to self-signed root` / `errSecInternalComponent` on some signatures — tama 0.1.30's `macos-code-signing` with no other scope open — while `security find-identity -v` read the identity as valid, and a Mac whose login keychain holds the intermediates signs every time (55167f6e). The intermediates now live in `~/.stado/signing/apple-issuers.keychain-db`, made once and never deleted, unlocked and kept on the user's keychain search list by every scope; the identity alone stays in the scope's temporary keychain. A failed `codesign` also reports `security verify-cert -p codeSign` on the leaf certificate, every Apple Worldwide Developer Relations certificate the search list holds with its SHA-1 and keychain, and the user's trust settings.

- **A superseded release run gives its builders back:** a submission that supersedes a live run of the same product and channel cancelled only that run's builds still waiting in the queue; a build already running was left to end unpublished, holding the builder's Cargo directory for the product the whole time, so the replacing run's build was declined there (`declined: job-…: a stado darwin-arm64 build is already running here and holds the Cargo build directory this one compiles into`) — stado 0.23.55's darwin build waited on 0.23.54's for its whole compile. A running build of a superseded run is now cancelled the way `stado cancel JOB_ID` cancels it (the cancellation fence its agent stops it on, then `cancelled/`), and the platform records `superseded by release run <id> (<product> <version>)`.

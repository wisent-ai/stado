# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.57](changelog/0.23.42-0.23.57.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A signature right after a release worker's no longer fails to build its chain:** the release worker signs with the fleet certificate and Apple's issuer chain from its environment, and the signer imported that chain into its temporary keychain and deleted it with the scope. Every scope that signed within the next moments on charless-mac-mini — tama's `test-product:oko:cli` install, seconds after tama's own `macos-code-signing` — failed `unable to build chain to self-signed root` with `errSecInternalComponent`, while `verify-cert`, `find-identity -v` and the search list all read fine, and the same install alone, or two signatures with no chain supplied, signed (55167f6e). Supplied issuers now go into the persistent `~/.stado/signing/apple-issuers.keychain-db`; a temporary keychain holds the identity alone.

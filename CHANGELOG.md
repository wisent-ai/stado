# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.52](changelog/0.23.42-0.23.52.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Signing scopes on one host take turns:** moving the temporary identity out of `~/Documents` (0.23.52) did not end `unable to build chain to self-signed root` / `errSecInternalComponent` on charless-mac-mini; tama 0.1.29's own `macos-code-signing` failed the same way under `~/.stado/signing` while `security find-identity -v` read the identity as valid and the user search list held its keychain. What the failures share is a second scope open at once on the host — a release worker's signing beside a product install or another build — each importing the same fleet identity into its own keychain on the one search list and deleting it when done (55167f6e). Every scope now waits for, then holds, the host's lock `~/.stado/signing/scope.lock` from making its keychain until it has deleted it, and a waiting scope prints `waiting for …/scope.lock: held by pid <pid> (running) since <time>: <command>`. The 0.23.52 entry's reading that `~/Documents` caused the failure was wrong; the identity stays under `~/.stado/signing` because that is where the scopes meet.

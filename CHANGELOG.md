# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.19](changelog/0.22.18-0.22.19.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `com.wisent.stado` takes the API listener over from a predecessor unit that declares no storage variables. The takeover resolves the predecessor's root the way its own process does: `WC_STORAGE_BACKEND`/`WC_LOCAL_STORAGE_PATH` from its environment, else the `storage` section of the config file it starts with (`STADO_CONFIG`, then the candidates under its `HOME`), else `local` and `~/.stado/local-storage`. Before, a first-generation unit such as `com.wisent.always-on.stado-object-api`, written without those variables, was reported as serving backend `None` root `None`, the takeover failed, and `stado serve --api` refused to start under the one unit on every host that still loaded it. A predecessor that resolves to another root is still refused, and the refusal now names the backend, the root and where it came from.
- A predecessor whose primary backend is the client route `stado` (or `stado-object`) is taken over like a `local` one: that route addresses an object API, and the API the predecessor's own process runs serves `storage.local.path`, which is how an operator's machine shares the fleet's one registry. Before, the takeover compared the backend word to `local` alone and refused `com.wisent.always-on.stado-object-api` on lukasz-macbook with `it serves backend "stado"` for the very root the one unit was about to serve. A backend that serves no local root (`s3`, `azure`, `gcs`) is still refused.

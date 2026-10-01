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
- A registry whose rollout strategies predate `readiness_poll_seconds` is read again. The field became required on 2026-10-01 without a declaration in the canonical registry, so every reader of the registry — `stado registry set`, `stado registry pull`, `stado release agent`, the `release` component of `stado serve` and with it `com.wisent.stado` — refused with `registry.release_control: missing field readiness_poll_seconds`, including the one command that could have declared it. A strategy that does not declare the field now reads 5 seconds until the operator declares one with `stado registry set --path release_control.products.<product>.strategy.readiness_poll_seconds --value <seconds>`; a declared `0` is still refused by validation.
- A registry whose resolver adapters still carry `idle_seconds` or `connect_seconds` is read again. Both fields were retired on 2026-10-01 while the canonical registry still declared `connect_seconds` on charless-mac-mini's adapters, so the resolver role refused the authority document with `registry.targets[charless-mac-mini].service_resolver: unknown field connect_seconds`, `stado serve --resolver` stopped, and with it `com.wisent.stado` and every `stado://` adapter on the host. The two fields are accepted and ignored.
- `stado bootstrap --local` folds a unit that already runs `stado serve` under another label into `com.wisent.stado`. The first-generation `com.wisent.always-on.stado-object-api` ran `stado serve --worker --disk-cleanup --resolver --release-interval-seconds` that way; the merge counted only single-role units (`stado work agent`, `stado resolver serve`, …) as resident roles and left it as it is, so the one unit came up with none of those roles and the host's resolver adapters went dark. Every role the predecessor runs is now a role of the one unit; an option both declare differently is refused naming the unit, the option and both values; a predecessor worker without `--poll-seconds` is refused with the `stado service ensure stado --host <host> --from <stado> --arg=serve --arg=--worker --arg=--poll-seconds=<N> …` declaration to make instead.

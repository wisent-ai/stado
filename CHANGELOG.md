# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

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

- Credential, scheduling, deployment and release-diagnostic explanations describe their rules without private incident timelines or identifiers. Diagnostic fields, policy decisions and command arguments are unchanged.

- `stado service release` takes `--readiness-url` and `--readiness-timeout-seconds` together (cli.md rule 14). The readiness window was 30 s whenever a URL was named without one; now naming either without the other is refused by the parser with exit 2. Pipeline promotion already reads the window its release policy declares.
- `stado host user create --registry-source` takes `remote`, `local` or `auto`; `gcs` is gone (cli.md rule 14). It never meant Google Cloud Storage: it read the canonical registry from whichever store `WC_STORAGE_BACKEND` selects, which `remote` now says.
- `stado azure` is now `stado cloud` (cli.md rule 14): `stado cloud login --provider azure …` and `stado cloud repair-rbac --provider azure …`, with the same flags as before. A call without `--provider` is refused by the parser with exit 2. The operator console allows the `cloud` family instead of `azure`, and `examples/providers/enable-azure.sh` ends with `stado cloud repair-rbac --provider azure` (it ended with a bare `stado azure`, which runs nothing).
- `stado credentials item apple-profile` is now `stado credentials item signing-profile --provider apple` (cli.md rule 14), with `--credentials` required: it read the `wisent-apple-notary` item when none was named. Stado Desktop's capability operation "Store code-signing provisioning profiles in a signing item" runs the new argv and no longer pre-fills a key item.
- `stado agent` and `stado serve` no longer list a host on Vast at a built-in $0.50 per GPU-hour for at most 3600 s (cli.md rule 14). When the Vast bridge runs, `--vast-price-gpu` and `--vast-max-duration-s` are required like `--vast-idle-window-s` (0 still means open-ended), and a missing one is refused with the flag named. A unit rendered by `bootstrap` carries them only when they were given; units already carrying `--vast-price-gpu=0.5 --vast-max-duration-s=3600` keep running with those values.

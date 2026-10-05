# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.36](changelog/0.23.12-0.23.36.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado local-control-plane` and `stado cloud-control-plane` are removed:** they were hidden ports of the retired Python `deploy.local_control_plane`/`deploy.cloud_control_plane`, and every role they bundled is a role of `stado serve`. No fleet host ran them; the one fleet entry named `local-control-plane` is a coordinator in the registry, which `serve --coordinator` runs. A device that belongs to no fleet now runs `stado serve --standalone --worker --kind local --poll-seconds <s> --control-plane local --control-plane-interval-seconds <s> --api --bind <addr> --port <port>`: `--standalone` runs the worker without a registry target and is refused beside `--resolver`, `--release-interval-seconds`, `--target` and `--auto`. Stado Desktop's **This Mac** backend starts that line and waits for `stado serve: target=` in its error log; its cloud container image runs `stado serve --control-plane cloud … --api`. The host installer no longer merges units running the removed commands, and its unused `coordinator` and `disk-cleanup` install kinds are gone.
- **A failed release can be resumed after its build job was reaped (5282f935):** `stado release submit` of the same source, `stado release resume` and `stado build` refused a failed platform whose job record the run reaper had already deleted with `build job <job> was not found in recorded states; refusing a replacement without terminal failure`, so the same commit needed a new version. The job's retained receipt (`status/<job>/output/receipt.json`), or failing that its run's retained outcome in `runs/<run>.json`, now says how it ended, and a failed or cancelled build is built again under a new identity. Only a job with no record, no receipt and no retained outcome is refused, with `build job <job> is in no queue state, left no receipt and no reaped run retains it`.
- **`stado cancel` of a job pinned to a fleet host no longer answers infra_down (94a07941):** the cancellation fence records a pinned job's agent reference (`local@<host>`) with no provider name, because such a job owns no provider resource, and reading the fence back refused that as `cancellation allocation … has invalid ownership fields` [infra_down, retry later]. An agent reference is accepted without a provider; a provider allocation still has to name its provider, and the refusal now prints the provider, instance and restarts it read.
- **The DNS MX preference read is an if/else (a050a5b9):** `bool.then(..).unwrap_or_default()` in `src/cli/dns/records/write.rs` failed clippy's `obfuscated_if_else` and with it the 0.23.36 release on both platforms; 0.23.36 was not published.

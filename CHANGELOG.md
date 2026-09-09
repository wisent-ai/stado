# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

When a release goes out, move its section into the newest file under
`changelog/`, and start a new range file when that one nears the limit.

## Released

- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Runner credential and registration:** runner operations resolve the same Skarbiec route that `runner credential` checks. Repository registration uses the existing repository-admin credential without demanding organization runner-group permissions. Installation reconciles changed scope, group and labels, refuses reconfiguration during a job, and requires GitHub to report the named runner online. Linux Rust toolchain setup can write its `.profile` without opening the whole runner home.
- **Runner desktop and model review:** Desktop's operator API now accepts the runner family, including read-only credential diagnostics. The host inspector displays GitHub registration results and all declared profiles. Brama model-review setup is an explicit `runner model-review TARGET --repository REPO` operation, separate from runner installation.
- **Release request diagnostics:** storage HTTP failures preserve the original cause chain when displayed by the CLI, recorded by a release run, or returned through the native Desktop command API. Connection errors no longer stop at `error sending request`; storage operations, schemas, and retry loops are unchanged.
- **Retained Tailscale diagnostics:** `host exec` reads the previous hour of macOS native Tailscale logs or the Linux `tailscaled` journal through two fixed, read-only commands. Hosts in Desktop exposes the same reads with the selected platform, original messages, process status, and refusals; neither empty output nor a completed read is a public Funnel health verdict. These two reads preserve up to 16 MiB of command stdout for a decodable native receipt, while other command limits and mutation confirmations remain unchanged.
- **Managed target builds:** `host build TARGET --manifest-path PATH --bin NAME` runs only `cargo build --locked --release` against a manifest physically confined below the approved account's `~/.stado/work/runs`, using the fleet's existing Cargo-path policy. The JSON receipt retains Cargo stdout, stderr, and its own exit status; symlinks, foreign ownership, missing tools, and paths outside the managed run area fail closed.
- **Attached target programs:** `host run-attached` connects stdin and, outside JSON mode, both output streams directly to one owner-executable program below the same managed run area. SIGHUP, SIGINT, and SIGTERM are delivered through a run-scoped owner-only marker and a second registry-authorized channel; sensitive input stays on stdin. `host remove-run-directory` idempotently removes one complete direct child of the managed run root and refuses the shared root, nested paths, symlinks, and foreign ownership.
- **Release archive length:** a release object read through `/api/release/object` is compared against the `Content-Length` the route declared, the way the authenticated object route already was. A body that stops short is refused with both counts named instead of being returned as the archive; a body with no declared length is still delivered whole.
- **Storage authentication refusals name storage:** `StorageError::Auth` reads `storage authentication failed`. Every authenticating backend raises it — the Stado object API's token file, Azure's token exchange, GCS credentials — so a refusal about a local token file's mode no longer announces `GCP authentication failed` on a machine with no GCP configuration. The error code, retryability and exit status are unchanged.
- **Release submit reports why nothing was built:** when no builder can claim a platform, the run's own refusal survives to the operator and to the durable run document. The signing-key read that follows an empty submission no longer replaces `no live fleet builder can CLAIM release_platform …` with `cannot read signing key …`.
- **Repair refuses an undeclared host:** `repair --target` for a host the canonical registry does not declare refuses with the registry's own sentence and a non-zero exit, instead of reporting an `unavailable` observation and exiting 0.
- **The doctor stops conflating two silences:** a unit file this machine never read and a unit file that belongs to another host are reported as different observations, and a product with no release target for a host says so rather than reading as agreement.

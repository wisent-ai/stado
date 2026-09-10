# Product boundaries

What the 0.5 product contract covers, what it deliberately leaves out, and
what each provider integration is allowed to claim. The README links here
rather than carrying it, because the boundary is what a reader checks before
adopting Stado and it changes on its own schedule.


## Included in the 0.5 product contract

- a provider-neutral queue and job lifecycle;
- local workers on registered workstations and servers;
- prompt-free preparation and observed readiness of the signed Apple code-capture
  helper on a registered Mac, through both the CLI and the native Hosts screen;
  [Apple preparation](https://stado.wisent.com/docs/desktop#prepare-apple-code-capture)
  documents the separate read-only and `--apple-only` actions;
- local filesystem queue/storage as the stable 0.5 execution path;
- GCS, S3, and Azure Blob queue/storage adapters released as preview until
  their release-scoped live sandbox suites pass;
- ephemeral VM lifecycle adapters for GCP, Azure, and AWS released as preview
  until their release-scoped live acceptance suites pass;
- externally managed Box capacity and Vast-host execution with the capability
  limits reported by `stado capabilities`;
- leases, compare-and-swap writes, fencing, pause, drain, recovery, and
  storage migration;
- immutable artifacts, result manifests, lineage, and scoped secret
  references;
- cost, capacity, quota, inventory, health, and ownership evidence where the
  selected provider adapter declares support;
- a human CLI, versioned machine JSON interface, native macOS Desktop, and
  read-only MCP interface; Desktop uses `WisentDesignSystem` from
  `wisent-ai/wisent-components` for its visual tokens and SwiftUI primitives;
- a loopback mobile-egress proxy whose upstream sockets are pinned to a named
  tether interface and whose process lifecycle is managed as a Stado service.

## Explicit non-goals for 0.5

- Stado is not a general Kubernetes replacement or a container platform.
- Stado does not manage arbitrary networks, load balancers, registries, or
  application platforms.
- Stado does not promise identical capabilities for every provider.
- Azure VM Scale Sets and AWS Auto Scaling are planned, not supported managed
  compute adapters.
- GCP managed instance groups are partial and are not part of the stable 0.5
  contract.
- Local hosts are attached and scheduled; Stado does not provision physical
  machines or install their operating systems, GPU drivers, or workload
  runtimes.
- Stado does not make an optional provider, alert channel, dashboard identity
  provider, or artifact service mandatory for local execution.

## Supported environments

The release manifest is authoritative for binary support. The initial release
matrix targets:

| Role | Platform | Status |
|---|---|---|
| Control plane and local agent | macOS arm64 | supported candidate; stable local scope |
| Control plane and local/cloud agent | Linux amd64 | supported candidate; cloud adapters remain preview |
| Control plane and agent | Linux arm64 | not yet supported |
| Workload GPU runtime | NVIDIA/AMD or CPU-only | supplied by the worker host or immutable workload image |

Cloud adapters require operator-provisioned accounts, networks, identities,
quotas, and storage. Stado mutates only resources admitted by its configured
ownership and policy boundaries.

## Capability status

`stado capabilities --json` is the source of truth for the installed build.
Its statuses have precise meanings:

- `implemented` — the adapter code implements the declared contract;
- `partial` — only the stated subset is available;
- `external` — Stado consumes or observes a dependency but does not manage it;
- `planned` — the capability is not available;
- `unsupported` — no contract exists.

An implementation status does not promote an integration to stable. Stable
provider support additionally requires the live acceptance evidence described
in the release documentation.


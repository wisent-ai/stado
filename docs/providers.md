# Providers

Which clouds can Stado dispatch to, how is a provider enabled, and what does
each one contribute? This page covers provider selection, quota, live VM
inspection, and the Vast.ai host-listing bridge. Flag-level detail lives in
[cli](cli.md).

## Selection: `WC_PROVIDERS`

The compute provider list is `providers` in the selected `STADO_CONFIG`
profile, overridable with the comma-separated `WC_PROVIDERS` environment
variable. The order is the preference order. The default is empty: an
unconfigured deployment has no provider rather than a hidden GCP dependency.

`providers_disabled` (env `WC_DISABLED_PROVIDERS`) is the explicit fence. A
fenced provider stays visible in configuration — the profile can explain why
a provisioned provider exists — but the scheduler never calls it.

The coordinator resolves this list into tick arms and, per provider and per
tick, checks running jobs, reaps dead agents, and schedules queued jobs. A
provider whose constructor fails (credentials missing) is logged and skipped,
so a misconfigured provider never blocks the primary one. `local` is in the
list but has no cloud tick arm: device-local agents claim assigned jobs
directly, and there is no VM lifecycle to schedule or reap.

Inspect the full catalog — capability families, variants, providers, and the
active selections — with:

```bash
stado capabilities
stado capabilities --json
```

The catalog answers what a user can ask Stado to provide and which providers
implement, partially support, expose externally, or plan each family
(compute, workload execution, object storage, quota and capacity, billing,
inventory, observability, and the rest). Compute is only one facet: the
storage backend is selected separately by `WC_STORAGE_BACKEND`, not by
`WC_PROVIDERS`.

## What each provider contributes

A cloud entry in `WC_PROVIDERS` contributes dispatch of paid agent VMs, and —
per the capability catalog — quota reads, machine pricing, and inventory to
the extent its adapter implements them. `local` contributes no provisioning
at all: it attaches existing registered hosts, whose capacity comes from the
GPU probe, free VRAM, and agent slots. `box` leases externally managed
fixed-shape machines and reports account limits and available boxes. `vast`
is not a dispatch provider: Stado is the host on Vast.ai, not the renter, and
renter-side provisioning is not implemented.

| Provider | What Stado uses it for | Where credentials live |
|---|---|---|
| `gcp` | Google Compute Engine VM dispatch and reap; live accelerator quota; machine prices, BigQuery billing export, credits, budgets; GCP asset inventory | Platform metadata identity where available, otherwise the `stado-gcp` Skarbiec item |
| `azure` | Azure VM dispatch and reap; configured quota reservations; machine prices, balance, usage, billing health; Stado-owned VM inventory | Azure managed identity preferred, then the `stado-azure` service-principal Skarbiec item |
| `aws` | Amazon EC2 VM dispatch and reap; machine-price estimation; Stado-owned EC2 inventory | `stado-aws` Skarbiec item (`access_key_id`, `secret_access_key`); IMDSv2 workload identity on adapter hosts without a grant; environment credential chains are disabled |
| `box` | Leasing externally managed fixed-shape boxes; account limits and available boxes | `stado-box/api_key` in Skarbiec |
| `local` | Executing on existing registered hosts; no machine provisioning | Registry SSH host keys in the selected credential store (`stado fleet key`); no OpenSSH-file fallback |
| `vast` | Listing our own idle GPU host on the Vast.ai marketplace | `stado-vast/api_key` in Skarbiec |

Cloud locators and credentials are never caller overrides: an enabled adapter
receives its exact profile and provider-plugin identity, and a workload-agent
grant must never contain `stado-gcp`, `stado-azure`, or `stado-aws`. See
[configuration](configuration.md) for the credential store itself.

## Quota

`stado quota` inspects GPU quota and submits increase requests across every
provider in `WC_PROVIDERS`. The default subcommand is `show`: live cloud
quota minus reservation minus running, per provider. A provider whose quota
fetch returns nothing — credentials absent, SDK not installed — appears as an
empty entry rather than vanishing, so a missing adapter is distinguishable
from zero quota. Iteration follows `WC_PROVIDERS`, so the picture matches
what job scheduling actually considers each tick.

```bash
stado quota show
stado quota catalog
stado quota request nvidia-tesla-t4 --to 16
stado quota requests
```

`request` submits one quota-increase request per (provider, region) via the
provider's Quotas API; regions default to every region the provider
dispatches into, and GCP requires a reviewer contact email
(`$WC_QUOTA_CONTACT_EMAIL` or `--email`). `request-all` covers every known
GPU family, `requests` shows cross-provider in-flight requests and support
communications, and `azure-replies`/`azure-escalate` answer open Azure quota
support tickets. Why quota shapes dispatch decisions is covered in
[jobs](jobs.md) and the spend side in [costs](costs.md).

## Live instances

`stado instances list` shows every live agent VM across the configured
providers and flags the ones no queue job or lease still references.
`--provider` narrows to one provider; `--json` emits machine-readable output.

```bash
stado instances list
stado instances list --provider gcp --json
```

Reaping itself is not a manual command: the coordinator's per-provider tick
reaps dead agents on every pass, including the scheduled
[autonomy](autonomy.md) ticks, so the list is an inspection surface, not the
enforcement mechanism.

## Renting out idle GPUs on Vast.ai

On GCP, Azure, and AWS, Stado is the renter. On Vast.ai it is the host: the
operator owns the GPU box and lists it on the marketplace so external renters
use otherwise-idle capacity when there is nothing to dispatch.

```bash
stado vast status
stado vast list
stado vast unlist
stado vast auto-list --dry-run
```

`list` publishes the configured machine at a per-GPU-hour price; it requires
`stado-vast/api_key` in Skarbiec and `WC_VAST_MACHINE_ID` unless the machine
can be discovered automatically. `unlist` removes every offer, blocking new
renters without terminating existing rentals. `status` shows Vast's current
view of the machine, and `monitor` is a one-shot snapshot of the bridge plus
wisent-compute state. `auto-list` is the daemon form: it polls the configured
Stado queue and the host's capacity blob, lists after the fleet has been idle
for the configured window (default 300 s), unlists when work appears, and can
cap the maximum rental length a renter can buy (default one hour). Existing
rentals are never preempted — only new renters are blocked.

## The provider-neutral rule

Every command above enters through Stado. Provider diagnostics belong inside
the corresponding adapter and are unavailable unless that provider is
explicitly enabled in the selected profile. Nothing falls back to a cloud
CLI, provider SDK, direct bucket URL, ambient credential, or a different
backend: `stado overview` resolves only the configured Stado backend and
enabled adapters, and no bootstrap, health, recovery, or release path invokes
`gcloud`, `gsutil`, or `az`. The operational consequences are in
[operations](operations.md).

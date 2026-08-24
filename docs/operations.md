# Operations

## Host health publication

Linux and macOS writers collect local disk and service state, then call
`stado host publish-beacon FILE`. The command requires
`STADO_HOST_HEALTH_API_URL` plus the dedicated
`stado-host-health-beacon` Skarbiec URL/consumer/grant metadata. It resolves
only `stado-host-health-api/token` and sends the document to authenticated
`PUT /api/host-health`. The control plane stores `host_health/<host>.json`
through its configured Stado backend, so Azure and local outage profiles write
to Azure Blob and local storage respectively.

Missing routing, an unreadable or over-broad grant, an insecure non-loopback
HTTP URL, failed authorization, and backend errors all leave the prior beacon
untouched and return failure. There is no cloud CLI, provider SDK, direct
bucket URL, ambient credential, or cross-backend fallback in the writer.

The systemd unit reads non-secret API/Skarbiec origins from
`/etc/stado/host-health.env`; the launchd template carries the same non-secret
routing metadata. Both keep the opaque Skarbiec grant owner-only at
`~/.stado/host-health-beacon-skarbiec-token`.

## Missing service reconciliation

The coordinator runs a service reconciliation stage during every
`stado optimize run` and scheduled autonomy tick. It joins two independent
facts: the unit state in the newest host beacon and a fresh
`stado service verify` reachability sweep from the declared consumer hosts.
The result is written to both
`state/autonomy/services/latest.json` and an immutable
`state/autonomy/services/runs/<timestamp>.json`; `stado optimize status` prints
the latest report.

Every autonomy object is rooted under `state/` because the object gateway
authorizes a write by matching its key against the configured namespace's
prefix allowlist. No namespace declares `autonomy/`, so the whole layer's
writes were refused with `401 unauthorized or non-immutable release write`
while the `local` backup backend kept serving stale reads — the same defect
`state/host_silence/` was moved to fix.

The fleet-wide host-silence threshold is also the service-beacon freshness
threshold. A missing or stale `reported_at` changes the service state to
`unknown`; it never authorizes a host mutation — with one exception. The
beacon unit's own death is what makes every other unit `unknown`, so a silent
host's declared beacon unit is reasserted through the idempotent
`service ensure` path over the host channel: the channel answering is the
evidence that repair is possible, `ensure` restarts in place and never
unloads, and once the beacon publishes again the rest of the host becomes
repairable from real evidence. A `failed` unit is the same repair as a
missing one — the unit exists, nothing runs under it, and the kick is in
place. For a host-probed unit, a live process running a stale copy of its own
declared binary is kicked (the four-day stale-agent incident), while a
process executing a binary the unit never declared stays refused as
`identity_unresolved`. For a fresh beacon that omits a declared unit:

| Endpoint evidence | Reconciliation |
|---|---|
| `observed` | Probe the declared unit. Stado adopts a corrected path or unit record only when the unit is loaded and its live process matches the declared program. If ownership cannot be proven, Stado records `identity_unresolved`, alerts once on the transition, and refuses to create a duplicate. |
| `unreachable` | Run the existing idempotent `service ensure` path. It creates a missing unit or restarts it in place, never unloads it, verifies the running postcondition, and updates the registry when the host selected a different valid unit path. |
| not declared in the service directory | The unit has no endpoint to disprove, so the host channel is the evidence: the unit is probed on the box, a loaded unit must prove its live program before adoption, and only a unit the host itself reports absent is ensured. |
| `unverified` | Record `endpoint_unverified`, alert once on the transition, and make no change because endpoint absence was not proven. |

Every repair renders its unit through the same resolution chain
`service ensure` uses: the host's registry declaration, then the shipped
Wisent catalog, then the declaration bundled with the build. A declaration
that names only a unit path cannot be reinstalled from the document; the
repair records `declaration_incomplete` and alerts until the registry entry
carries its `program` and `args`, which is the durable fix — read the truth
with `stado service show <name>`, write it into the entry, and every future
repair renders from the document.

`AutonomyMode::Report` records the same plan without executing it.
`EnforceSafe` and `EnforceOwned` execute the reversible repair actions,
bounded by `max_actions_per_tick`, the emergency pause, the circuit breaker,
and a per-service mutation lease. Only a mutation that failed on a host feeds
the circuit breaker; `declaration_incomplete` and `identity_unresolved` are
refusals computed before any host command runs, and a refusal must not starve
the healthy repairs behind it. Recovery-managed units stay with the fixed
host-recovery program and are never silently converted into registry
services; the beacon exception asserts the unit without writing the registry
for them.

Known gap: a timer-driven oneshot beacon (Linux `host-health-beacon.timer` →
oneshot service) is a unit shape `service ensure` cannot yet express — it
asserts a running unit, and a oneshot exits by design. Declaring it as a
plain service would create a restart loop, so such hosts stay with their
installed timer and `scripts/publish-linux-host-beacon.sh`.

Useful operator views:

```bash
stado service list
stado service verify
stado optimize status
stado optimize run
```

Every command below enters through Stado. Provider diagnostics belong inside
the corresponding adapter and are unavailable unless that provider is
explicitly enabled in the selected profile.

## Isolated GitHub pre-check runners

The runner lifecycle enters only through Stado:

```bash
stado host precheck-runner install <registry-target>
stado host precheck-runner status <registry-target>
stado host precheck-runner remove <registry-target>
```

Stado resolves the host address and `release_platform` from the canonical
registry. `install` exchanges `GITHUB_TOKEN.value` through Stado's
admin-scoped Skarbiec coordinates for a short-lived organization registration
token, transports the installer and token on host-channel stdin, verifies the
pinned Actions Runner archive, and installs the OS service. It also makes the
Brama host's Skarbiec reconcile missing routes from its live vault, resolves
`agent:probierz` to the request-signing item and field selected by Skarbiec,
reads that field on the host without putting its value in argv, and installs the
resolved value through stdin as the runner-owned, mode-`0600`
`.stado/kronika-agent-auth-secret`; the non-secret `probierz` agent ID is
published as `routes/kronika-agent-id` beside `routes/brama.url`. Kronika runs
the audit, while Probierz is the Brama client identity authorizing that product
workflow. `remove` uses a short-lived removal token before deleting the service,
account, files, and network rule.

The runner has one unprivileged `stado-precheck` account and a root-owned
pre/post-job cleanup hook. Workspaces, diagnostics, package and toolchain
caches, application caches, and `.stado` are runner-owned; the rest of the
installation is root-owned and not writable by jobs. An nftables UID rule on
Linux and a PF user rule on macOS reject loopback, RFC1918, link-local,
unique-local, and CGNAT/Tailscale ranges while leaving public GitHub and package
endpoints reachable. The one exception is the exact loopback Brama port
published by Stado for the authorized `kronika` consumer. Those CIDRs are
protocol network classes compiled into Stado, not fleet host addresses; fleet
destinations remain registry data.

GitHub runner group `stado-precheck` grants access to an explicit repository
list. Stado keeps public-repository admission disabled until
`stado host precheck-runner repository-add <repository>` admits a named public
repository; those workflows must refuse pull requests whose head repository
differs from the base repository before GitHub assigns the job to this runner.
Eligible Linux jobs use `runs-on: [self-hosted, Linux, X64, stado-precheck]`;
eligible macOS jobs use
`runs-on: [self-hosted, macOS, ARM64, stado-precheck]`. Repository access is
the GitHub-side boundary: workflow-ref restrictions remain disabled because
same-repository pull-request jobs execute from `refs/pull/*`, not the default
branch.

## Common queries

### Fleet, queue, quota, and billing

```bash
stado overview
stado overview --json
```

`overview` resolves the configured Stado backend and enabled adapters. It does
not fall back to a provider CLI, ADC, or a different storage backend.

### Local agent state

```bash
stado host health <registry-target>
stado host inventory <registry-target>
stado host exec <registry-target> -- nvidia-smi
```

### Inspect one job end-to-end

```bash
scripts/watch_job.sh <job_id>
stado machine status <job_id>
stado machine logs <job_id> --cursor 0 --limit 1048576
```

## Failure mode quick-grep

The most common failure modes — search the per-job stdout for one of
these substrings to classify failures fast:

| Substring | Cause |
|---|---|
| `HfHubHTTPError: 429` | HF Hub rate limit (free tier 1000 req / 5 min). Retry path lives in `wisent.core.utils.infra_tools.infra.data.dataset_splits.get_all_docs_from_task` and the cache fast-path in `generate_pairs_from_task.py`. |
| `Couldn't find cache for` | datasets cache miss for a config that doesn't exist on the dataset's HF repo. lm-eval task config drift. |
| `OverflowError: int too big to convert` | tokenizer `model_max_length` was a sentinel (1e30) handed to the rust binding's u32. Capped at 4096 in `activations_collector.py`. |
| `Dataset scripts are no longer supported` | datasets 4.x dropped the script loader. Pinned `datasets<4.0` in the agent template. |
| `huggingface-hub>=0.34.0,<1.0 is required` | transformers 4.55.x dep-pin mismatch. Pinned `huggingface-hub<1.0` in the agent template. |
| `RuntimeError: Cannot set NUMBA_NUM_THREADS` | numba init happened before wisent's env-set. Set `NUMBA_NUM_THREADS=1` in the agent's env BEFORE Python starts. |
| `does not appear to have files named` | transformers shard-name miscompute on `gpt_oss` / 0-indexed safetensors. Fixed in `transformers>=4.57`. |
| `AttributeError: ... has no attribute 'transformer'` | wisent activation hook expected GPT-2 path on a model whose `model_type` contains `gpt`; gpt_oss uses Llama-style. Fixed in `transformer_analysis.py`. |
| `gated repo` / `401 Client Error` | The scoped workload credential cannot read the requested repository. Rotate `stado-huggingface/token` through the stdin-only Skarbiec service path; never place the token in VM metadata or logs. |
| `Quota 'PREEMPTIBLE_NVIDIA_*_GPUS' exceeded` | Hit the regional preemptible quota. Either raise via GCP console or add zones to `MACHINE_TYPE_ZONES`. |

## Release / publishing

Release operations enter through Stado:

```bash
stado release catalog sync --root /path/to/registered-checkouts
stado release catalog audit
stado release submit --source /path/to/product --version <exact-version> \
  --channel candidate
```

Submit requires a clean committed tree but does not contact its remote. It
archives the exact committed tree, publishes the create-only source object,
records source and manifest identity in the Stado catalog, and creates one
provider-neutral queue job pinned to a registry builder whose
`release_platform` matches the recipe. Queue state and
`status/<job>/output/` remain the authoritative work and transport records.

Inspect or resume a run by repeating the same submit command. Its identity is
derived from product, version, channel, source digest, and manifest digest; the
durable `stado://release-runs/<id>/run.json` shows job IDs, output coordinates,
delivery state, and failure. A terminal successful platform output is read
from JobStorage and published, never rebuilt.

The release authority is configured by item name and trusted key ID:

```text
release.signing_key_item = stado-release-signing
release.signing_key_id   = stado-release-2026-08
```

The Skarbiec key-pair item contains the base64 PKCS#8 value in `private_key`. Key bytes stay in
Skarbiec. Build and delivery secrets are only checked-in `item#field`
references and must also be permitted by `agent.skarbiec.secret_fields`.

Canonical publication is immutable and ordered:

```text
release.tar.gz -> qualification.json -> release.sig -> release.json
```

The signed manifest is the commit marker. Required delivery jobs run only
after it exists and consume its exact URI and digest. Optional mirrors do not
gate canonical success. Runtime products then use the existing
`registry.release_control` generation CAS and release-agent reconciliation.
`deployment.json` is written only after every declared target reports the
promoted version, artifact digest, and manifest digest exactly.

No Git forge credential, hosted workflow, provider repository checkout, or
direct provider API is required by this chain. Existing external adapters may
mirror completed releases, but their availability cannot change source,
qualification, signing, desired state, or observed rollout truth. See
[`release.md`](release.md) for the strict manifest and catalog contract.

## Bringing up a new local box

```bash
# Install a verified Rust release and resolved profile first.
./install-stado.sh
export STADO_CONFIG="$HOME/.stado/config.json"
export STADO_TARGET="<registry-target>"

# The provider-neutral installer validates config and preflight, then delegates
# persistent launchd/systemd ownership to Rust bootstrap.
./install.sh

# Health publication additionally requires the dedicated
# stado-host-health-beacon Skarbiec grant and non-secret Stado/Skarbiec origins
# described above. It never requires a cloud login.
stado host health "$STADO_TARGET"
```

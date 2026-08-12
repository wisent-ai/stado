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
token, transports the installer and token on host-channel stdin, verifies the pinned Actions
Runner archive, and installs the OS service. `remove` uses a short-lived
removal token before deleting the service, account, files, and network rule.

The runner has one unprivileged `stado-precheck` account and a root-owned
pre/post-job cleanup hook. Only `_work` and `_diag` are writable by jobs. An
nftables UID rule on Linux and a PF user rule on macOS reject loopback,
RFC1918, link-local, unique-local, and CGNAT/Tailscale ranges while leaving
public GitHub and package endpoints reachable. Those CIDRs are protocol network
classes compiled into Stado, not fleet host addresses; fleet destinations
remain registry data.

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

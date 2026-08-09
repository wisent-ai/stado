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
ssh root@<host> '
  systemctl is-active wisent-agent.service
  journalctl -u wisent-agent.service --since="5 minutes ago" --no-pager -o cat
  ps -eo pid,etime,cmd | grep extract_and_upload | grep -v grep
  nvidia-smi --query-gpu=memory.used,memory.free,utilization.gpu --format=csv,noheader
'
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

`.github/workflows/deploy.yml` is dispatched for the committed default-branch
revision. The one run gates the committed Cargo version, creates or
safely resumes `v<version>`, publishes both platform archives, bootstraps the
control plane, promotes stable desired state, and invokes fleet reconciliation.

Every `stado://releases/stado/<version>/<platform>/<file>` write goes through
the authenticated Stado release API. The workflow expands only
`stado-release-publisher/token` through its dedicated, sole-item Skarbiec
grant and always requests create-if-absent.

A retry reads back an existing object and accepts it only when the bytes are
identical. Different bytes at the same version/platform URI are a hard
collision; they are never overwritten. There is no PyPI workflow, provider
CLI upload, ADC path, mutable image tag, or `latest` release pointer. See
[`release.md`](release.md) for channels, manifests, compatibility, promotion,
upgrade, and rollback.

Install an exact release before service bootstrap:

```bash
export STADO_API_URL=https://stado.wisent.com
export STADO_RELEASE_VERSION=<exact-immutable-version>
export STADO_RELEASE_PLATFORM=<exact-release-platform>
./install-stado.sh
```

The installer downloads `release-manifest.json`, verifies product, version,
platform, every artifact SHA-256, and `SHA256SUMS`, preserves prior binaries,
then replaces the selected release atomically. Service deployment remains a
separate explicit step.

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

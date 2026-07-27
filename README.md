# wisent-compute

Job queue and compute management for Wisent GPU workloads.

`wisent-compute` runs a fleet of GPU workers — long-lived local agents plus
auto-scaling cloud agents — against a single GCS-backed job queue. The Rust
Cloud Run control plane schedules work continuously; Rust agents claim jobs
that fit their available VRAM. The system includes priority queues,
per-accelerator zone rotation, pair-text caching, cost-aware dispatch, and
condition-driven idle shutdown for cloud VMs.

## Install

Released Rust binaries are published to
`gs://wisent-compute/releases/stado/`. Install or refresh a registered host
with:

```bash
./deploy/stado-up.sh <target>
```

The primary binaries are `stado` and its compatible `wc` entry point. The
coordinator runs as the authenticated `stado-coordinator` Cloud Run service.

## Quick start

```bash
# 1. Submit a single job (any GPU consumer with capacity will claim it)
wc submit "python -m wisent.scripts.activations.extract_and_upload \
  --task gsm8k --model 'meta-llama/Llama-3.2-1B-Instruct' \
  --device cuda --layers all --limit 32"

# 2. Submit a batch (one command per line)
wc submit --batch jobs.txt --spot --max-cost-per-hour 4.00 ''

# 3. Watch progress
wc status

# 4. Pull results from GCS once a job completes
wc results <job_id> ./out/

# 5. Run the local agent on a workstation (polls queue, claims jobs that
#    fit in nvidia-smi-detected VRAM)
wc agent --auto

# 6. Run a one-shot scheduling tick locally
wc coordinator --once

# 7. One operational snapshot: jobs, fleet, quota, budgets and credit burn
stado overview
```

## Registry-controlled disk cleanup

Local targets can opt into bounded cleanup through their canonical GCS registry entry. Cleanup fails closed when the registry is unavailable, invalid, stale, or does not uniquely match the local hostname. Start every rollout in `report` mode; switch to `enforce` only after inspecting the host report.

```json
"disk_cleanup": {
  "mode": "report",
  "check_interval_seconds": 300,
  "low_free_gb": 30,
  "target_free_gb": 60,
  "max_bytes_per_pass": 42949672960,
  "max_items_per_pass": 20,
  "max_scan_items": 10000,
  "cleaners": {
    "huggingface_cache": {"min_age_seconds": 604800}
  }
}
```

`wc disk-cleanup --once` performs one policy-controlled check; use registry `mode: "report"` for a read-only pass. `wc disk-cleanup --watch` follows the registry interval. On the local Mac, `wc install-disk-cleanup` installs the watch as a launchd LaunchAgent. Only complete, old, exclusively referenced Hugging Face cache revisions are eligible; active compute slots, held cache locks, unknown layouts, scan/deadline caps, and path or ownership changes block deletion.

## Documentation

- [`docs/cli.md`](docs/cli.md) — full CLI reference (`wc submit`, `wc agent`, `wc coordinator`, `wc registry`, `wc cost`, `wc bootstrap`).
- [`docs/architecture.md`](docs/architecture.md) — data flow, scheduling rules, cloud-agent VM lifecycle, the GCS layout (`queue/`, `running/`, `completed/`, `failed/`, `capacity/`).
- [`docs/configuration.md`](docs/configuration.md) — every `WC_*` / `GCP_*` env var, the registry schema, the live-quota + reservation overlay, GCP one-time setup.
- [`docs/operations.md`](docs/operations.md) — common operator queries (failure breakdowns, fleet inspection, log paths) and release/publishing flow.

## Project layout

```text
stado-rs/
  src/                                # Rust CLI, queue, scheduler, agents, providers
  data/                               # registry, profiles, startup templates
  cloudbuild.yaml                     # release binaries and coordinator image
deploy/
  deploy_stado_rust.sh                # Cloud Run production deployment
  stado-up.sh                         # binary installer for registered hosts
  gcp_setup.sh                        # one-time GCP bootstrap
.github/workflows/
  deploy.yml                          # push-to-main Rust release and deployment
  registry-bootstrap.yml              # registry changes install Rust agents
```

## Contributing

Build and release from `stado-rs/`. The Cloud Build pipeline publishes the
Linux binaries, checksum manifest, stable release pointer, and coordinator
container image. Production deployment is handled by
`deploy/deploy_stado_rust.sh`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).

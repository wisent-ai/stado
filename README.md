# wisent-compute

Job queue and compute management for Wisent GPU workloads.

`wisent-compute` runs long-lived local agents and autoscaling cloud agents
against the queue and object backend selected by `STADO_CONFIG`. The Rust
control plane schedules work continuously and Rust agents claim jobs that fit
their available VRAM. Azure is the active production backend; the local
profile provides authenticated outage operation. Cloud-provider adapters are
explicit opt-ins rather than bootstrap defaults.

## Install

Released Rust binaries come from the configured immutable Stado release
channel. Install or refresh a registered host with:

```bash
./deploy/stado-up.sh <target>
```

The primary binaries are `stado` and its compatible `wc` entry point. The
coordinator runs through the configured authenticated Stado control boundary.

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

# 4. Pull canonical results through Stado once a job completes
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

Local targets can opt into bounded cleanup through their canonical Stado registry entry. Cleanup fails closed when the configured backend is unavailable, invalid, stale, or does not uniquely match the local hostname. Start every rollout in `report` mode; switch to `enforce` only after inspecting the host report.

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
- [`docs/architecture.md`](docs/architecture.md) — provider-neutral data flow, scheduling rules, agent lifecycle, and canonical object prefixes.
- [`docs/configuration.md`](docs/configuration.md) — deployment profiles, authenticated boundaries, provider fencing, registry schema, and quota overlay.
- [`docs/operations.md`](docs/operations.md) — Stado operator queries and immutable release publication.

## Project layout

```text
stado-rs/
  src/                                # Rust CLI, queue, scheduler, agents, providers
  data/                               # registry, profiles, startup templates
deploy/
  deploy_stado_rust.sh                # native coordinator bootstrap
  stado-up.sh                         # pinned immutable binary installer
.github/workflows/
  deploy.yml                          # push-to-main Rust release and deployment
```

## Contributing

Build from `stado-rs/`. The deploy workflow publishes each version/platform
object through the exact `stado-release-publisher` Stado boundary with
create-if-absent semantics. A retry accepts only byte-identical objects; a
different payload at an existing URI is a release collision. Installers require
an explicit version and never resolve a mutable `latest` image or pointer.

## License

Apache-2.0. See [`LICENSE`](LICENSE).

# CLI reference

All commands accept `--help` for the canonical option list. The package
installs a `wc` entry point.

## `stado overview [--json]`

Single operator snapshot for the whole control plane: queue counts, fresh
worker heartbeats, registered/offline local targets, provider quota, the latest
GCP/Azure billing-health record, live GCP Billing budgets, seven-day credit
burn, and cumulative promotional credits applied. Human-readable output is the
default; `--json` returns the complete source document. GCP does not expose a
promotional grant ceiling, so remaining promotional credit is reported as
unavailable rather than inferred.

## `stado billing show|refresh [--json]`

`billing show` reads the latest coordinator snapshot. `billing refresh` queries
GCP and Azure immediately, prints the result even when the disabled source GCP
bucket cannot cache it, and attempts to publish the same document to
`billing_health/credits.json`.

Azure uses the Microsoft Customer Agreement credits endpoint and reports the
current and estimated balances, pending eligible charges, expired credit,
grant amount and validity when the billing property is readable, billing
status, and the paid-overage risk when the spending limit is off.

The Azure billing service-principal JSON is read exclusively from Azure Key
Vault. Configure the vault data-plane URL with `WC_AZURE_KEY_VAULT_URL` and the
secret name with `WC_AZURE_BILLING_SECRET` (default:
`wisent-azure-billing-sp`). Stado authenticates to the vault with Azure Managed
Identity or the current Azure CLI identity; it never bootstraps vault access
from a client secret. There is no credential fallback to a local file, process
environment, queue blob, or GCP Secret Manager.

The secret JSON uses lowercase fields `tenant_id`, `client_id`,
`client_secret`, `billing_account`, `billing_profile_system_id`, and optionally
`subscription_id`. The Azure principal needs Billing profile reader on the
selected billing profile. The identity running Stado separately needs
permission to read that Key Vault secret.

## `stado billing watch [--interval DURATION] [--once] [--json]`

Foreground billing watchdog. Each poll refreshes the snapshot, evaluates two
independent conditions, alerts on the transition into either, and prints a
status line. `--once` evaluates a single time and exits; otherwise it loops on
`--interval`, a duration string such as `45s`, `5m`, `2h` or `1d` (default
`5m`).

Run it **outside the cloud it monitors**. A collector that dies with its
provider cannot warn you about that provider: the coordinator's billing tick
runs as a Cloud Function inside the same GCP project it measures, so when that
project's billing was shut off the collector was shut off with it and
`billing_health/credits.json` simply stopped changing. A laptop, a host in
`registry.json`, or another cloud all work.

The two conditions:

- **Credit balance** — the existing thresholds. These are only computable
  while a provider section reports `ok`, because `credit_depleted` and
  `available_balance` exist only in the success branch of a query.
- **Account health** — a provider section reporting `no_credentials` or
  `error` for longer than the grace period, which is exactly what a closed
  account, a revoked billing export or a disabled service principal looks
  like. The alert names the provider, how long it has been broken, and the
  exact upstream cause. Per-provider last-good timestamps are folded forward
  inside `billing_health/credits.json` under `account_health`, so "how long
  has this been broken" is answerable from the snapshot alone.

Alerts fire on the transition into a condition, not on every poll. The firing
set is persisted with the snapshot, so de-duplication survives a restart of
the watchdog and is shared with a concurrent coordinator tick. Recovery is
printed and logged, never alerted.

Recent provider billing mail is surfaced as advisory evidence, using the same
read-only Gmail client as `stado mail`: providers announce closure, failed
payment and credit expiry by email days before the API starts refusing calls.
It is strictly best-effort — no Gmail token, no scope, or an unreachable Gmail
prints why the evidence is missing and never fails the watch.

## `stado mail search|analyze`

Read-only Gmail integration. Neither command sends, labels, archives, nor
deletes mail.

```bash
stado mail search --query 'from:microsoft.com azure'
stado mail analyze --query '\"Microsoft for Startups\" OR \"Azure credits\"' --json
```

`search` lists sender, subject, date, categories, monetary amounts, date
mentions, action status, and a Gmail link. `analyze` additionally aggregates
message counts, categories, unique monetary amounts, and messages requiring
action. Classification is deterministic; message text is not sent to an LLM.

Authentication uses `WC_GMAIL_ACCESS_TOKEN` (preferred),
`GOOGLE_OAUTH_ACCESS_TOKEN`, or gcloud Application Default Credentials
authorized for the `gmail.readonly` scope. Only metadata, snippets, extracted
signals, and links are emitted; full message bodies are not printed.

## `wc submit`

Submit a job (or a batch) to the GCS-backed queue.

| Option | What it does |
|---|---|
| `wc submit COMMAND` | Submit one shell command as a job. |
| `wc submit --batch FILE ''` | Submit each line of `FILE` as a separate job, in parallel via a `ThreadPoolExecutor`. |
| `wc submit --priority N` | Higher `N` is dispatched before lower `N`. Default 0. Tie-break inside a priority bucket is FIFO on `created_at`. |
| `wc submit --spot --max-cost-per-hour 4.00` | Dispatch on Spot/Preemptible at most $4/hr per accel. Set to 0 for no cap. |
| `wc submit --any-provider` | Default. Any consumer with capacity may claim. |
| `wc submit --pin-provider` | Only the requested `--provider` may claim. |
| `wc submit --provider gcp\|local` | Hint for which provider should pick the job up. With `--any-provider` this is just a hint. |
| `wc submit --gpu-type STR` | Pin the accelerator label (`nvidia-l4`, `nvidia-a100-80gb`, ...). Skips the `--model X.YB` regex inference. Machine type resolved from `GPU_SIZING` unless `--machine-type` is also passed. |
| `wc submit --vram-gb N` | Caller-declared VRAM requirement. Picks the smallest tier in `GPU_SIZING` whose memory >= N. Use this when the job's command has no `--model` substring (any non-wisent workload). |
| `wc submit --machine-type STR` | Pin the GCE/Azure machine type verbatim (`g2-standard-8`, `Standard_NC8ads_A10_v4`, ...). For SKUs not in the catalog. |
| `wc submit --pre-command STR` | Shell snippet placed before the command in the same bash shell — `export FOO=...` reaches the subprocess. Joined via `&&` so a non-zero exit in the prelude aborts the job. |
| `wc submit --apt PKG[,PKG...]` | Apt packages installed via `sudo -n apt-get install -y --no-install-recommends` before the subprocess spawns. Cloud-kind agents only; local-kind agents refuse the job. |
| `wc submit --output-uri gs://...` | Additional destination mirrored to after job completion. Additive — `status/<id>/output/` is always written too. |
| `wc submit --verify STR` | Post-success shell command. Non-zero exit reverses `COMPLETED → FAILED`. Catches silent-success failure modes (e.g. wisent's `extract_and_upload` reporting "5/7 strategies failed" but exiting 0). |

Submitter-side env: `HF_TOKEN`, `GH_TOKEN` are read from the submitter's
environment and baked into the per-job startup script that the cloud
agent renders at boot. `COMPUTE_API_KEY` (if set) routes the submission
through the `compute.wisent.com` HTTPS API instead of writing GCS
directly.

**Sizing precedence** (each layer overrides the previous):

1. `estimate_gpu_memory(command)` — model-name regex on the command, the wisent-eval default.
2. `--vram-gb N` — caller-declared VRAM, skips the regex.
3. `--gpu-type STR` — pinned accelerator, picks machine_type from the catalog.
4. `--machine-type STR` — pinned machine type verbatim.

If none of the GPU flags are set AND no `--model X.YB` matches, the job lands on `e2-standard-8` (CPU).

**Example** (Z-Image LoRA training on a fresh L4 with ai-toolkit deps):

```bash
wcomp submit \
  --gpu-type nvidia-l4 --vram-gb 22 \
  --apt libgl1,git-lfs,build-essential,libglib2.0-0 \
  --repo https://github.com/ostris/ai-toolkit.git --repo-workdir ai-toolkit --repo-extras "" \
  --pre-command 'TORCH_NVDIR=$(python3 -c "import os,nvidia; print(os.path.dirname(nvidia.__file__))"); export LD_LIBRARY_PATH=$(ls -d $TORCH_NVDIR/*/lib|paste -sd:):$LD_LIBRARY_PATH' \
  --output-uri "gs://wisent-images-bucket/Jakubs-lora/zimage_lora_run03_$(date +%F)/" \
  --verify 'gsutil -q stat gs://wisent-images-bucket/Jakubs-lora/zimage_lora_run03_*/checkpoints/zimage_lora_run03.safetensors' \
  "cd ai-toolkit && python run.py /opt/zimage-lora/configs/run.yaml"
```

Or, collapsed via a profile (see `wc profiles` below):

```bash
wcomp submit --profile ai_toolkit_zimage \
  --output-uri "gs://wisent-images-bucket/Jakubs-lora/zimage_lora_run03_$(date +%F)/" \
  "cd ai-toolkit && python run.py /opt/zimage-lora/configs/run.yaml"
```

## `wc profiles`

| Subcommand | Behavior |
|---|---|
| `wc profiles` | List available profiles with one-line descriptions. |
| `wc profiles NAME` | Print the profile's resolved JSON. |

A profile is a JSON file under `stado/profiles/` (bundled with
the wheel) or `$WC_PROFILES_DIR/` (operator-local override). It bundles
the `wc submit` flags for a recurring workflow — `gpu_type`, `vram_gb`,
`apt`, `pre_command`, `repo`, `repo_workdir`, `repo_extras`,
`output_uri`, `verify`, `priority`, `spot`, `max_cost_per_hour`,
`provider`, `pin_provider`. Every field is optional.

Discovery order:

1. `$WC_PROFILES_DIR/<name>.json` — operator-local; first hit wins.
2. `stado/profiles/<name>.json` — bundled with the package.

**Merge rule:** CLI flags always win. A kwarg that equals the
wisent-compute default (empty string / 0 / False / [] / "train" for
`repo_extras`) counts as "unspecified by CLI" and adopts the profile's
value. To override a profile field, pass the explicit flag.

**Bundled profiles:**

| Profile | What it sets up |
|---|---|
| `ai_toolkit_zimage` | Z-Image Turbo LoRA training via Ostris ai-toolkit. L4 (22 GB request), apt deps for cv2 + git-lfs + build tools, cu128/cu129 cuBLAS `LD_LIBRARY_PATH` fix as `pre_command`, ai-toolkit repo clone. |

To add a new bundled profile: drop a JSON file in `stado/profiles/` and bump the package version. To add an operator-local profile without a release: `WC_PROFILES_DIR=/path/to/profiles wcomp submit --profile mything ...`.

## `wc status [filter]`

Tab-separated table of queue / running / completed / failed jobs. With
`COMPUTE_API_KEY` set, hits `compute.wisent.com`; otherwise reads GCS
directly. Optional filter narrows by job-id or batch-id substring.

## `wc cancel <job_id> [--terminate]`

Remove a queued job from `gs://$WC_BUCKET/queue/<id>.json`, or terminate
a running instance via the provider's `delete_instance(...)` and move
the job to `failed/` with `error="cancelled"`.

Plain `cancel` reads the instance reference from the job document and
nowhere else, so the two states where a VM exists but the document does
not name it — a dispatch that created the instance and died before
stamping the job, and a job already rewritten by a partial cancel — leave
a machine running that nothing reclaims.

`--terminate` closes that. It resolves the reference from the job
document first and from `provider-leases/<job_id>.json` second, deletes
the instance through `providers::get_provider`, and prints which blob the
reference came from. A `local@<host>` reference is a local agent slot,
not a cloud resource, and is skipped. When neither record names an
instance the command says so and — for a job sitting in `running/`, which
by definition should be holding one — exits non-zero rather than
reporting a clean termination it did not perform. The job is still
cancelled in that case; the non-zero exit means "go look at the provider
console", not "nothing happened".

Without the flag, every call behaves exactly as it always has.

## `wc job rerun <job_id> [--json]`

Resubmit a finished, failed or cancelled job under a new job id, and
print `old -> new`.

The original is read from whichever lifecycle prefix holds it, including
`cancelled/` — the one prefix the bulk job listing does not walk. The
spec is then replayed through the ordinary submit path rather than
hand-written, so the startup script, the `runs/<run_id>.json` manifest
and the `gpu_mem_gb` / `priority` / `gpu_type` blob metadata that the
fitting-jobs listing prefilters on are stamped by exactly the code that
stamps them for a fresh `wc submit`.

Routing is pinned to what the original *resolved* to, not re-derived, so
the rerun lands on the same hardware. The exception is a job that came
out of the CPU branch (no accelerator, no sized VRAM, the default CPU
SKU): pinning its machine type back would make the submit path treat it
as a GPU request, so the rerun re-enters the same branch instead. A
command that still sizes to nothing comes out identical; one the fleet
has since measured gets its real size, because the original's zero
recorded "not sized yet", not "needs no GPU".

The new job carries `re_submission_of = <old id>`, which is what makes
the tombstone tracker write `fixed/<old id>.json` or
`failed_again/<old id>.json` when it terminates. `schedule_id` is not
carried: a manual rerun is not a scheduled submission.

## `wc job watch <job_id> [--follow] [--json]`

Print the job's command log from the beginning. With `--follow`, keep
polling for new bytes at the fleet's own poll interval until the job
reaches a terminal prefix, then print a status row and exit with the
job's outcome — non-zero for `failed` and `cancelled`.

This is `wc machine logs` wrapped into a tail. The byte cursor is carried
forward across polls, so each poll asks only for the bytes that appeared
since the last one and the stream never restarts at zero. A job whose
agent restarts the command re-uploads its log from the beginning; the
tail notices, rewinds and replays rather than dying.

`--json` buffers the log instead of streaming it and emits one object
with the normalized job, the terminal flag, the log length and the log
text. The outcome still travels in the exit status, and the diagnostic
goes to stderr, so stdout stays a single parseable object.

## `wc results <job_id> <dir>`

`gsutil -m cp -r 'gs://$WC_BUCKET/status/<job_id>/output/*' '<dir>/'`.

## `wc agent`

Run a long-lived GPU agent. Polls `gs://$WC_BUCKET/queue/`, claims any
job whose `gpu_mem_gb <= free_vram_gb` AND passes `_job_eligible`, spawns
the job as a subprocess, and tracks completion.

| Flag | Behavior |
|---|---|
| `wc agent --gpu-type X` | Override the broadcast SKU label (default: nvidia-smi auto-detect). |
| `wc agent --target NAME` | Pull `gpu_type` and `slots` from the registry by name. |
| `wc agent --auto` | Look up self in the registry by hostname. Re-fetches periodically so registry edits propagate without restarting the agent. |
| `wc agent --idle-shutdown` | Exit cleanly (and self-delete the GCE VM if running on one) when no slots active and no eligible queued job remains. Used by cloud-agent VMs. |

The agent broadcasts capacity to
`gs://$WC_BUCKET/capacity/<consumer-id>.json` every poll cycle. The
scheduler reads these broadcasts to decide whether to *yield* a job to
a free local consumer instead of paying for a fresh cloud VM.

## `wc coordinator`

Run the scheduling tick locally instead of as the Cloud Function.

| Flag | Behavior |
|---|---|
| `wc coordinator --target NAME` | Use the named coordinator entry from the registry. |
| `wc coordinator --once` | Run a single scheduling tick and exit (cron-friendly). |

Useful for development and for redundancy if the Cloud Function is
unavailable.

## `stado host`

| Subcommand | Behavior |
|---|---|
| `stado host health <target>` | Print the latest registry-managed host beacon, disk state, service states, log tail, and backing-object timestamp/generation. |
| `stado host health <target> --json` | Emit the same read-only report as JSON for automation and MCP. |
| `stado host reboot <target>` | Graceful reboot through the approved channel. Reports `reboot_requested` or the host's own refusal — usually sudo wanting a password. |
| `stado host uptime <target>` | Uptime, load averages and logged-in users. Load is read from the kernel, not scraped from the `uptime` line, whose shape differs between macOS and Linux. |
| `stado host ping <target>` | One verdict from two signals: ssh reachability and health-beacon age. The worse signal decides, so a box answering ssh with a stale beacon fails. |
| `stado host disk <target>` | Disk usage plus the registry cleanup policy and the janitor's own state: last pass, bytes freed, next scheduled pass. |
| `stado host cleanup <target> --dry-run` | Preview what the registry cleanup would delete. `--dry-run` is mandatory; it drives the janitor's own planning phase and writes no state. |
| `stado host exec <target> -- CMD` | Run one approved read-only command. An allowlist, not a shell: the operator's words select a fixed argv entry and never join the command line. A refusal prints the allowlist. |

Every one of these resolves its target from the canonical registry and
refuses a target that is unknown, not a local host, or has no registry-managed
ssh destination. They share one channel, `deploy/host_channel.rs`, which
derives its ssh options from `host reboot`'s rather than copying them, so the
commands cannot drift apart. All accept `--json`.

## `stado registry`

Every subcommand reads and writes the canonical registry through the store
`WC_STORAGE_BACKEND` selects: `gs://$WC_BUCKET/registry.json` on `gcs`,
the `registry.json` blob of the configured container on Azure, S3 or a
local root. Pinned to GCS, the repair path was unusable on the very
deployment that needed it.

| Subcommand | Behavior |
|---|---|
| `stado registry validate [path]` | Validate a local registry-v2 document without writing. |
| `stado registry push [path]` | Upload `stado/targets/registry.json` (or `path`), validated, compare-and-swapped, then read back and verified. |
| `stado registry pull` | Print the canonical registry to stdout. |
| `stado registry self [--name-only]` | Which registry target this machine is. |
| `stado registry doctor [--json]` | Diff registry declarations against live host state. Exits non-zero on any divergence. |
| `stado registry host add HOST --ssh DEST [--kind local]` | Onboard a machine into the registry, validated, refusing duplicates. |
| `stado registry beacon-age [--json]` | Every registry host and its last beacon, worst first. |

### `stado registry doctor`

Reads the host beacons (`host_health/`) and the capacity broadcasts
(`capacity/`) — never ssh — so it costs one prefix listing and is safe to
run on a loop. It reports four kinds of disagreement, and names each one
before exiting non-zero:

- `no-heartbeat` — a `kind=local` target with no beacon object at all.
- `stale-beacon` — a beacon past the fleet liveness window
  (`CAPACITY_STALE_SECONDS`), i.e. the publisher stopped.
- `missing-plist` / `unit-not-active` — a service the target declares is
  absent from the beacon's unit map, or present and not active.
- `unmanaged-host` / `unmanaged-agent` — a host publishing beacons, or a
  local agent broadcasting capacity, that no registry target claims.

An unreachable store is an error, never a clean report: "the registry
could not be read" and "the registry does not list you" drive opposite
decisions.

### `stado registry beacon-age`

One row per registry target, sorted worst-first: hosts that never reported
at all, then the oldest beacon, then the `gcp`/`vast` targets where no
beacon is expected. The "has not reported in days" detector.

## `stado storage`

| Subcommand | Behavior |
|---|---|
| `stado storage copy` | Copy queue state from one storage backend to another. Writes. |
| `stado storage ls [PREFIX]` | Per-prefix object counts across the canonical set, or the objects under one prefix. |
| `stado storage stat PATH` | One object: `present`, `absent` or `unreachable`, with size, timestamp, metadata and version token. |
| `stado storage cat PATH` | Write one object's body to stdout. |
| `stado storage verify` | Compare two stores object-for-object. Read-only. |

Everything except `copy` is read-only. `ls`, `stat` and `cat` inspect the
one store `WC_STORAGE_BACKEND` selects; `copy` and `verify` take both
stores as explicit flags.

**Absent is not unreachable.** During the GCP-billing outage nobody could
answer "is the queue empty, or is the store gone?", because the Azure
backend's `exists` maps every failure to `false` and its `updated_at` maps
every failure to `None` — a forbidden container and an empty one read
identically. None of the commands below use either method: `stat` probes
through the versioned download, which propagates the storage error, and
`ls` reports a prefix it could not list as `unreachable` rather than as a
count of zero.

### `stado storage copy`

Copy queue state from one storage backend to another — the migration path
for when the store behind the queue has to change, such as a GCS project
lost to a billing shutdown. Both ends are built from the flags alone and
never from `WC_STORAGE_BACKEND`, so the source and the destination can be
different kinds of store in the same process.

| Flag | Behavior |
|---|---|
| `--from gcs\|azure\|s3\|local` | Source backend. Required. |
| `--to gcs\|azure\|s3\|local` | Destination backend. Required. |
| `--from-bucket` / `--to-bucket` | Bucket for a `gcs` or `s3` end. |
| `--from-account` / `--to-account` | Storage account for an `azure` end. |
| `--from-container` / `--to-container` | Container for an `azure` end. |
| `--from-path` / `--to-path` | Root directory for a `local` end. |
| `--from-region` / `--to-region` | Region for an `s3` end; empty defers to the AWS default chain. |
| `--prefix P` | Restrict the copy to `P`. Repeatable. Omitted copies the whole canonical prefix set. |
| `--dry-run` | Print the per-prefix plan and write nothing. |
| `--concurrency N` | Objects copied in parallel. Defaults to the crate's bulk-download budget. |

```bash
stado storage copy \
  --from gcs --from-bucket stado \
  --to azure --to-account stadoprod --to-container stado \
  --dry-run
```

Every object is copied body first and then has its source metadata
re-applied, because the scheduler prefilters queued jobs on the
`gpu_mem_gb` / `priority` / `gpu_type` metadata keys before downloading any
job body — a body-only copy would quietly turn every scheduling tick into a
full queue download. The Azure backend swallows metadata write failures, so
the copier re-reads each destination prefix and reports by name every object
whose metadata did not land.

Nothing is ever deleted at either end, and the copy is resumable: a
`storage_copy/.copy.json` sentinel in the DESTINATION records the last
cleanly finished prefix, and an object already present at the destination
with the same size is skipped. Re-running after a clean pass walks
everything again to pick up churn. The command exits non-zero when any
object failed, and prints the per-prefix tally of copied / metadata-repaired
/ skipped / vanished / failed objects plus the bytes written.

The canonical prefix set is `queue/`, `running/`, `completed/`, `uploaded/`,
`failed/`, `cancelled/`, `queue_priority/` (including its `.migration.json`
sentinel), `scripts/`, `status/`, `capacity/`, `provider-leases/`, `runs/`,
`fixed/`, `failed_again/`, `schedules/`, `cancellations/`,
`machine_requests/`, `machine_inputs/`, `config/`, `state/`,
`failure_fixes/`, `coverage/`, `host_health/`, `billing_health/`,
`hf_rate/`, `artifacts/` and the root object `registry.json`. `cancelled/`
is spelled out because it is not one of the prefixes the job listing walks,
which makes it the easiest prefix to lose in a hand-rolled copy.

**Drain the fleet first.** Copying a live queue produces split-brain — a job
claimed from the old store, written to the new one, and reaped from neither.
`deploy/MIGRATE_TO_STADO.md` documents the ordering: stop the coordinator
tick and every agent, confirm there are no queued and no running jobs, copy,
then cut over.

### `stado storage ls [PREFIX] [--limit N] [--size] [--json]`

With no `PREFIX`, prints one row per canonical prefix with its object
count — the fast answer to "is the queue actually empty?". A prefix that
could not be listed shows `UNREACHABLE` with the storage error instead of
a count, and the command exits non-zero, so an unknown count is never
rendered as zero.

With a `PREFIX`, lists the objects under it with their update timestamp
and metadata. A listing failure is an error, not an empty table.

| Flag | Behavior |
|---|---|
| `--limit N` | Maximum objects listed under an explicit prefix. Defaults to the largest count one byte can express; the output says when it truncated. |
| `--size` | Also report each object's body size. Opt-in: the backend listing carries name, timestamp and metadata but no size, so this costs one download per listed object (bounded by `--limit`). |
| `--json` | Emit the same report as JSON. |

```bash
stado storage ls                      # per-prefix counts
stado storage ls queue/ --limit 20    # the 20 lexically first queued jobs
```

### `stado storage stat PATH [--json]`

Reports one object as `present`, `absent` or `unreachable`, together with
its size, `updated_at`, metadata and backend version token (GCS
generation, Azure ETag, local content hash).

The probe is the versioned download, never `exists`: `exists` cannot
distinguish "the object is not there" from "the store did not answer".
A body that is not UTF-8 (a collected artifact) is re-probed as bytes, so
it reports `present` with no version token rather than a false failure.
Metadata comes from the listing, which is a separate grant from object
read; if listing is denied while the read worked, the report says so and
the object still reads as `present`.

**Exit code.** Zero means the question was ANSWERED — both `present` and
`absent`. Non-zero means it was not: `unreachable`. Branch on the exit
status for "do I know?", and on the `state` field for "is it there?".

```bash
stado storage stat registry.json --json
stado storage stat queue/7f3c1c2a.json
```

### `stado storage cat PATH`

Writes one object's body to stdout, unchanged. Job documents,
`registry.json`, the health beacons and the queue-control blob are all
JSON an operator reads directly. An absent object and an unreachable
store are separate errors, and neither prints an empty body.

```bash
stado storage cat registry.json | jq '.targets | keys'
```

### `stado storage verify --from … --to …`

The post-copy check `deploy/MIGRATE_TO_STADO.md` demands ("verify object
counts match") and never provided. Takes the same locator flags as
`stado storage copy` — `--from`, `--to`, the per-end bucket / account /
container / path / region flags, and a repeatable `--prefix` — plus
`--json`. It reads both stores and writes to neither; it never re-copies
anything.

Per canonical prefix it compares object counts, the names present on only
one side, and the metadata keys that did not land at the destination. The
metadata rule is the copier's: keys folded to lowercase (Azure
round-trips them through case-insensitive headers, GCS does not), empty
values ignored (Azure filters them out before the write, so they can
never land), and extra destination keys allowed because both backends
merge on write.

A side that cannot be listed is reported as unknown — `?` in the count
column — never as empty. The command exits non-zero on ANY divergence and
names every diverging object, so the tail of the output is the work list.

```bash
stado storage verify \
  --from gcs --from-bucket stado \
  --to azure --to-account stadoprod --to-container stado
```

## `wc cost`

| Subcommand | Behavior |
|---|---|
| `wc cost report` | Per-target / per-model `$` spend computed from completed jobs (`started_at` → `completed_at` × spot or on-demand `$/hr` per accel). |
| `wc cost estimate <batch>` | Project total `$` for a batch file using observed per-job cost from completed-jobs history. |

## `wc bootstrap`

| Flag | Behavior |
|---|---|
| `wc bootstrap [--target NAME]` | SSH into the registry-named host and install + enable the agent as a systemd unit. |
| `wc bootstrap --local` | Install on this machine via launchd (macOS) or systemd-user (Linux) instead of via SSH. |
| `wc bootstrap --dry-run` | Print the unit/plist; do not enable. |

## `wc host user create`

Creates an idempotent local account on SSH-managed `kind=local` registry
targets. The command supports macOS and Linux, creates a standard account by
default, and requires either one or more explicit `--target` values or `--all`.
The initial password is prompted without echo and sent only through SSH stdin;
it is never included in the local SSH argument list or registry.

```bash
# Inspect the selected hosts without connecting.
wc host user create controlyourai-relay --all --dry-run

# Create a standard GUI-capable account on one host.
wc host user create controlyourai-relay \
  --target charles-mac \
  --full-name "ControlYourAI Relay"
```

Use `--admin` only when the account requires administrator privileges.
`--require-password-change` expires the initial password at first login.
Remote SSH users must be root or have non-interactive `sudo`; an existing
account is reported as `exists` and is not modified.

## `stado queue`

Maintenance mode. One small blob (`config/queue_control.json`) that the
coordinator tick and every agent re-read as they run.

| Subcommand | Behavior |
|---|---|
| `pause [--reason TEXT]` | Stop dispatching and stop new claims. |
| `resume` | Start dispatching and claiming again. |
| `status [--json]` | Pause flag, reason, since, who, plus `queue`/`running` job counts. |
| `drain [--wait] [--timeout SECS]` | Pause, then optionally wait for `running/` to empty. |

A pause stops exactly two things: the coordinator creating instances, and
agents claiming NEW queued work. It does not touch jobs already running —
they keep their slot, keep heartbeating and finish normally — and it does
not cancel anything: queued jobs keep their place and dispatch resumes
where it left off. Cron schedules still fire; their jobs simply wait in
`queue/` with the rest.

That asymmetry is what makes a drain terminate. `drain --wait` polls
`running/` and returns only once it is empty, printing how many jobs are
left on each poll. If `--timeout` elapses first it exits non-zero and the
queue stays paused. The default timeout is the heartbeat-staleness window
(`config::HEARTBEAT_STALE_MINUTES`), after which anything still running is
long work rather than a slot about to be reaped.

```bash
# The pre-migration sequence deploy/migrate_to_stado.sh assumes.
stado queue drain --wait
stado queue status --json
stado storage copy --from gcs --to azure ...
stado queue resume
```

`CONFIRM_FLEET_DRAINED=yes` in `deploy/migrate_to_stado.sh` is an
honour-system flag; `stado queue drain --wait` returning zero is what
makes it true. Copying a live queue produces split-brain — a job claimed
from the old store, written to the new one, and reaped from neither.

## `stado service`

Full service management for the units registry hosts run. The group exists
because a wedged `com.wisent.weles-api` once sat on a mac mini that Stado
could reach, restart and recover — but not manage, because nothing declared
the unit. `host recover` reloads a fixed list of agents; this group manages
an arbitrary, per-host, declared set.

| Subcommand | Behavior |
|---|---|
| `list [--json]` | Every managed service on every host, with its state. |
| `status NAME [--json]` | One service everywhere it is managed. |
| `restart NAME [--host TARGET] [--json]` | Restart one unit; no recovery pass. |
| `adopt UNIT --host TARGET [--json]` | Bring an existing unit under management. |
| `retire UNIT --host TARGET [--json]` | Bootout/disable and forget; files kept. |
| `deploy NAME --host TARGET --from PATH [--json]` | Render, push, bootstrap, record. |
| `logs NAME [--host TARGET] [--lines N] [--json]` | Tail the unit's log. |
| `env NAME [--host TARGET] [--json]` | Effective environment, secrets redacted. |

`NAME` accepts either the logical service name or the host's own name for
the unit (launchd label, systemd unit name), so `service restart weles-api`
and `service restart com.wisent.weles-api` are the same request. Omitting
`--host` acts on every host that manages the name.

### Where the managed set comes from

A service is managed when it is declared, and there are two declarations:

- **`registry`** — the `services` array on the target's registry entry.
  `adopt`, `retire` and `deploy` edit exactly this, through the validated
  write path (`stado registry push`'s own read/validate/compare-and-swap/
  read-back). A mutation that would produce an invalid registry document is
  refused with nothing uploaded.
- **`recovery`** — the fixed agent list `stado host recover` reloads on
  every pass. Those units really are managed, so they are listed, but they
  are managed by that fixed program rather than by the registry document:
  they cannot be retired. Adopting one is allowed and is the way to make
  its management explicit.

### State comes from the beacons

`list` and `status` read the `host_health/` beacons and nothing else — no
ssh, no per-host round trip — so the fleet-wide answer survives the host
being the broken thing.

| State | Meaning |
|---|---|
| `active` / `inactive` / `failed` | What the host's latest beacon reported. |
| `missing` | The beacon exists and does not carry this unit at all. |
| `unknown` | The host has published no beacon, or reported no state. |

`missing` and `unknown` are deliberately different answers: a silent host
is not the same fact as a vanished unit, and treating them alike is how a
dead box reads as a healthy one.

### Everything else rides one ssh channel

`restart`, `adopt`, `retire`, `deploy`, `logs` and `env` go through
`deploy/host_channel.rs`, whose option set is derived from `host reboot`'s
rather than re-typed (`BatchMode=yes`, `ConnectTimeout`,
`StrictHostKeyChecking=accept-new`). The remote program is fixed per
command and reports through the tab-delimited `STADO_*` markers
`host recover` established; registry data never becomes a shell fragment.

`deploy --from PATH` takes the absolute path, **on the target host**, of
the program the unit runs. The plist / systemd unit is rendered by the same
renderer `stado bootstrap --local` uses, so a remotely deployed service is
byte-identical to a locally installed one. Both spellings travel in one
program and the host picks, so a deploy costs one round trip and never
guesses the remote init system.

`env` parses the unit's own plist or unit file and redacts every value
whose variable name looks like a credential, in the table and in `--json`
alike. It over-redacts on purpose: a name like
`GOOGLE_APPLICATION_CREDENTIALS` holds a path and is redacted anyway,
because the alternative is an allowlist whose first wrong entry prints a
live token. systemd `EnvironmentFile=` references are reported but not
read, so a partial picture says so.

```bash
# The weles-api gap, closed.
stado service list
stado service adopt com.wisent.weles-api --host charless-mac-mini
stado service restart com.wisent.weles-api
stado service logs com.wisent.weles-api --lines 40
```

## `stado doctor [--json] [--fix-hints]`

Ordered deployment preflight. Every check reports PASS, WARN or FAIL with a
remedy naming the exact variable or command that fixes it, and the command
exits non-zero if anything FAILs.

| Check | What FAIL means |
|---|---|
| config | Backend, providers, locator and the config file actually in use. FAILs on the azure backend with an empty storage account; WARNs when the container is still the default, which silently reads an empty container. |
| storage | Writes, reads back and deletes one probe object. This is the check that separates "the queue is empty" from "the store is unreachable". |
| providers | An authenticated call per configured provider, reporting the upstream error verbatim. |
| quota | Live quota per accelerator. FAILs when everything is zero, because dispatch then cannot succeed no matter what else is right. |
| release | Fetches the release pointer the agent VMs install themselves from. Unreachable means cloud-init aborts before the agent starts. |
| template | Renders the agent startup template through the dispatcher's own code path and asserts no placeholder survives. A preflight that renders differently would prove nothing. |
| vm-identity | Azure VMs with no managed identity can neither read the queue nor delete themselves. |
| registry | Reachable, parses, and names this host or an active coordinator. |
| queue-control | Reports a paused queue, which otherwise looks exactly like an idle fleet. |
| alerts | At least one channel, and not only the GCP one on a deployment with no GCP. |

Checks are fault-isolated and individually deadline-bounded: one failure never
prevents the rest from running, and a black-holed endpoint becomes one FAIL row
instead of a hung command. The probe object is deleted even when the read-back
fails; a leaked one is named so it is obviously safe to remove.

## `stado instances list|reap`

Live agent VMs across the configured cloud providers, and the reaper for the
ones nothing owns.

| Subcommand | Behavior |
|---|---|
| `stado instances list [--provider P] [--json]` | Instance reference, provider, accelerator, age, and an `orphan` column. A VM is an orphan when no running job and no unexpired provider lease references it. |
| `stado instances reap [--provider P] [--older-than 2h] [--dry-run] [--yes]` | Deletes orphans. Dry run is the DEFAULT; deletion needs `--yes`, and an explicit `--dry-run` overrides it. Referenced VMs are never deleted and the skip names the holder. |

A provider that cannot be enumerated makes the command exit non-zero rather
than render as "no orphans" — a hole in the inventory is exactly the failure
this command exists to prevent. `--older-than` takes a duration (`30m`, `2h`,
`1d`); a bare number is rejected.

## `stado secrets put|get|ls|rm`

Application credentials in Azure Key Vault. `put` reads the value from STDIN
only — there is deliberately no `--value` flag, because argv is visible in
process listings and shell history:

```bash
stado secrets put wisent-azure-billing-sp < sp.json
```

`get` is the only subcommand that renders a value; `ls` reads metadata alone.
The billing collector reads its Azure service principal from here and nowhere
else — no fallback to another cloud's secret manager, because that coupling is
what turned a closed GCP billing account into a loss of Azure credit visibility.

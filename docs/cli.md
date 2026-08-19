# CLI reference

All commands accept `--help` for the canonical option list. The package
installs a `wc` entry point.

## Failures and exit codes

Every command that fails prints three things and then exits.

```
Error: GCS API error HTTP 503: backend unavailable
infrastructure we depend on is unreachable — our failure [infra_down]; retry later
2026-07-27T18:22:03.114Z ERROR stado::failure: infrastructure we depend on is unreachable
  failure_point="cli.status" error_code="infra_down" service="queue" retryable=true
  severity="critical" detail="GCS API error HTTP 503: backend unavailable"
```

The first line is the command's own message, verbatim and unabridged — these
are operator tools, and hiding the upstream body or the variable name from the
person who has to fix it would only cost them a round trip. The second line is
the classification. The third is the structured record a log shipper picks up;
its fields (`failure_point`, `error_code`, `service`, `retryable`) are the same
ones every other Wisent service writes. Nothing on this path makes a network
call: a CLI that phones an analytics collector while the network is already
suspect has simply acquired a second way to hang.

`error_code` is the ecosystem failure contract's code set:

| Code | Meaning | Whose problem |
|---|---|---|
| `config` | Our deployment configuration is incomplete or wrong — an unset variable, a tool that is not installed. | Ours |
| `auth` | The credentials the command used were rejected. | Yours |
| `not_found` | The job, object or target named does not exist. | Yours |
| `rate_limit` | An upstream is throttling us, or a quota is exhausted. | Ours |
| `timeout` | An upstream did not answer in time. | Ours |
| `infra_down` | Storage, a provider API or the network is unreachable. | Ours |
| `unknown` | The failure could not be attributed. | Unclear |

An infrastructure failure is never reported as `not_found`, and never as a
clean exit. When a message carries an upstream HTTP status, that status decides
the code, so a 503 mentioning "not found" in its body still classifies as
`infra_down` — collapsing 5xx into "nothing there" is what once let a storage
outage read as an empty queue.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | The command ran and failed, and running it again will not help: `config`, `auth`, `not_found`, `unknown`. |
| `2` | Usage error — bad arguments, or a command that is not implemented. Argument errors exit here regardless of what the message reads like. |
| `69` | `sysexits.h` `EX_UNAVAILABLE`. The failure is transient: `infra_down`, `timeout`, `rate_limit`. Retrying later can work. |

`69` is the one code that was added, and it is the only signal a retry loop
needs; `0`, `1` and `2` mean exactly what they always did. The split is
retryability, not blame — a rate limit is ours but waiting clears it, while a
missing environment variable is also ours and no amount of retrying will fix
it. Commands with their own richer exit contract keep it: `storage stat`
still distinguishes "the question was answered" from "it was not", and
`job watch` still exits with the job's own outcome.

```bash
# The retry wrapper this makes possible.
for attempt in 1 2 3; do
  wc submit --profile default -- python train.py && break
  [ $? -eq 69 ] || exit $?   # not transient: stop, do not hammer it
  sleep $((attempt * 30))
done
```

The same classification backs `stado`'s alert delivery: when a Slack, Telegram,
SendGrid or Pub/Sub channel cannot deliver, the failure is logged as a
structured row with `failure_point="monitor.alerts.deliver"` and never
propagated — an alert must not be the thing that kills the process it is
reporting on — so a silently dead alert path is now something a log query
finds rather than an absence someone has to notice.

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
the sources selected by `billing.providers` (or `WC_BILLING_PROVIDERS`);
the default is `["gcp", "azure"]`. Source selection is independent from compute
provider enablement, so a fenced account can remain monitored without allowing
the scheduler to provision into it. The command attempts to publish the result
to `billing_health/credits.json`.

Azure uses the Microsoft Customer Agreement credits endpoint and reports the
current and estimated balances, pending eligible charges, expired credit,
grant amount and validity when the billing property is readable, billing
status, and the paid-overage risk when the spending limit is off.

The Azure billing service-principal object is read exclusively from the
separate `skarbiec` repository/service. `WC_SKARBIEC_URL` addresses the local
Stado resolver adapter (default `http://127.0.0.1:17602`), never a physical
Skarbiec host. `WC_SKARBIEC_CONSUMER` selects the scoped consumer (default
`stado-control-plane`), and `WC_SKARBIEC_TOKEN_FILE` selects its owner-only
grant file (default `~/.stado/control-plane-skarbiec-token`). Raw grants are not accepted
from environment variables.
`WC_AZURE_BILLING_SECRET` selects the item
id (default `wisent-azure-billing-sp`). There is no credential fallback to
Azure Key Vault, a local credential file, process environment, queue storage,
or another cloud's secret manager.

The item is a canonical Skarbiec bundle with lowercase fields `tenant_id`,
`client_id`, `client_secret`, `billing_account`, `billing_profile`,
`billing_profile_system_id`, and optionally `subscription_id`. The Azure
principal needs Billing account reader plus subscription Billing Reader on the
selected billing profile and subscription. The Stado consumer grant needs an
exact `read:wisent-azure-billing-sp#<field>` capability for every field above.
An HTTP 204 from the balance endpoint is a successful query with
`balance_reported: false`; billing-property grant and subscription state remain
available and authoritative.

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

Authentication is resolved only through the scoped `stado-gmail` Skarbiec
item, using either its short-lived access token or centrally stored OAuth
refresh fields. The command never reads ambient OAuth variables, ADC, or a
cloud CLI session. Only metadata, snippets, extracted signals, and links are
emitted; full message bodies are not printed.

## `wc submit`

Submit a job (or a batch) to the store selected by the authoritative Stado profile.

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
| `wc submit --output-uri stado://NAMESPACE/KEY` | Additional provider-neutral object destination mirrored after job completion. Additive — canonical job output is always written too. |
| `wc submit --verify STR` | Post-success shell command. Non-zero exit reverses `COMPLETED → FAILED`. Catches silent-success failure modes (e.g. wisent's `extract_and_upload` reporting "5/7 strategies failed" but exiting 0). |

Workload credentials are declared as `secret_env` item/field references and
resolved through the scoped Skarbiec agent grant. Raw values are never read
from submitter environment variables or embedded in a job document or VM
startup metadata. The compute API credential is also resolved from Skarbiec.

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
  --output-uri "stado://wisent-images/training/zimage-lora/run03" \
  --verify 'test -s output/checkpoints/zimage_lora_run03.safetensors' \
  "cd ai-toolkit && python run.py /opt/zimage-lora/configs/run.yaml"
```

Or, collapsed via a profile (see `wc profiles` below):

```bash
wcomp submit --profile ai_toolkit_zimage \
  --output-uri "stado://wisent-images/training/zimage-lora/run03" \
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
`apt`, `pre_command`, `repo`, `repo_ref`, `repo_workdir`, `repo_extras`,
`output_uri`, `verify`, `priority`, `spot`, `max_cost_per_hour`,
`provider`, `pin_provider`. A repository requires `repo_ref` as its full
lowercase commit hash; branches, tags, short hashes, and missing refs fail
before queue creation.

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

`wc status` reads the canonical queue through the backend resolved by
`STADO_CONFIG`; infrastructure failures remain distinct from absent jobs.
The optional filter narrows by job-id or batch-id substring. It has no direct
GCS or legacy `COMPUTE_API_KEY` path.

## `wc cancel <job_id> [--terminate]`

Remove a queued job from the canonical configured store, or terminate a
running instance through its enabled provider adapter and move the job to
`failed/` with `error="cancelled"`.

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

Downloads canonical output through the configured Stado `BlobBackend`; it
never shells out to a provider CLI or exposes a provider-native locator.

## `wc agent`

Run a long-lived GPU agent. It polls the queue through the store selected by
the authoritative Stado profile, claims an eligible job, spawns it as a
subprocess, and tracks completion.

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
| `stado host publish-beacon <file-or-dash>` | Validate and publish one locally collected beacon through the route-scoped authenticated Stado API. It requires the dedicated host-health Skarbiec grant and has no direct-storage or provider credential fallback. |
| `stado host reboot <target>` | Graceful reboot through the approved channel. Reports `reboot_requested` or the host's own refusal — usually sudo wanting a password. |
| `stado host gpu-power-limit <target> <watts>` | Persist the NVIDIA board power cap on the canonical registry target, apply it immediately over the approved host channel, and report the driver's effective limits. The local agent reasserts the declaration every five minutes and after restart; it advertises zero free capacity when reconciliation fails. |
| `stado host uptime <target>` | Uptime, load averages and logged-in users. Load is read from the kernel, not scraped from the `uptime` line, whose shape differs between macOS and Linux. |
| `stado host ping <target>` | One verdict from two signals: ssh reachability and health-beacon age. The worse signal decides, so a box answering ssh with a stale beacon fails. |
| `stado host disk <target>` | Disk usage plus the registry cleanup policy and the janitor's own state: last pass, bytes freed, next scheduled pass. Also names the host's local APFS snapshots, which hold space no stado command reclaims. |
| `stado host cleanup <target> --dry-run` | Preview what the registry cleanup would delete. `--dry-run` is mandatory; it drives the janitor's own planning phase and writes no state. |
| `stado host gates <host>` | Why this host is claiming nothing: the blockers its own agent publishes, the disk policy behind them, and its declared slots against its published free ones. Read-only. Exits non-zero when the host is not claiming. |
| `stado host reclaim <host> [--dry-run\|--apply --reason TEXT]` | Reclaim disk in four declared stages — the host's own janitor pass, the release build scratch tree, delivered product trees no `current` link and no live process references, and the macOS per-launch Chromium code-sign clones. Previews by default; `--apply` needs `--reason` and appends an audit record on the host. |
| `stado host exec <target> -- CMD` | Run one approved read-only command. An allowlist, not a shell: the operator's words select a fixed argv entry and never join the command line. A refusal prints the allowlist. |
| `stado host inventory <target>` | The stado-managed binaries under `$HOME/.stado/bin`, the `$HOME/.stado/forwards/*.url` markers, the listening loopback TCP ports, the Skarbiec vault files under `$HOME/.stado` as metadata, whether the installed `stado` knows a fixed set of subcommands — and, the point of the command, whether each forward marker still matches a live listener. |
| `stado host software [<target>] [--json]` | What a host actually runs: one row per program with its version, its SHA-256 and whether those exact bytes came out of a release Stado published. Naming a target takes the report over the audited channel and persists it as an observation; omitting the target prints what every host has already reported, ages included. The read counterpart of `host release`, and the evidence `stado release status` gates on. |
| `stado host release <target> --binary NAME --version X.Y.Z` | Put one registry-declared managed binary on the host: fetch the exact coordinate, verify the operator's configured SHA-256, check the layout, stage it under a versioned directory, and only then atomically repoint the active binary and restart its declared unit. The write counterpart of `host inventory`. `--dry-run` probes read-only and reports the plan. |
| `stado host install-binary <target> --from PATH [--name NAME]` | Replace one owner-only Stado program on the host with a build proven to run there. It is delivered over the approved channel, signed, executed BEFORE it becomes the installed one, renamed into place rather than written through the file already there — overwriting a Mach-O in place invalidates its signature and the kernel answers the next exec with SIGKILL and no message — verified again, and the previous build is kept. `--rollback` puts that previous build back. |
| `stado host precheck-runner install <target>` | Install or reconcile the isolated GitHub pre-check runner declared by Stado. The target address and platform come from the canonical registry; Stado obtains a short-lived organization token from the admin-scoped `GITHUB_TOKEN` credential, verifies the pinned runner archive, creates an unprivileged account, installs the service, and applies the private-network boundary. |
| `stado host precheck-runner status <target>` | Read the service, runner identity, and nftables/PF boundary through the same registry-authorized channel. |
| `stado host precheck-runner remove <target>` | Deregister the runner with a short-lived removal token, stop and delete its service and files, remove its account, and remove its network boundary. |
| `stado host sync-acquisition-scopes <target> <source>` | Register the checked-in Skarbiec acquisition-scope catalog on the host. The local catalog file is delivered into `$HOME/.stado/files` through the `host install-file` channel — owner-only, checksummed on arrival — and an embedded fixed script derives the Ed25519 workload public key from the host's `weles-credential-workload-private.pem` (replacing an older key only after registration with its successor succeeds) and runs `skarbiec token-register-acquisitions --replace-capabilities` against the host's fleet vault, then prints the reconciled status. Nothing is installed on the host, and a refusal carries the remote's own words. |

Diagnostic and recovery commands resolve their target from the canonical registry and
refuse a target that is unknown, not a local host, or has no registry-managed
ssh destination. They share one channel, `deploy/host_channel.rs`, which
derives its ssh options from `host reboot`'s rather than copying them, so the
commands cannot drift apart. All accept `--json`.

### `stado host precheck-runner`

This is the complete lifecycle; there is no Python installer or separately
installed diagnostic helper. `install`, `status`, and `remove` all resolve
`<target>` through the canonical registry and select Linux x86-64 or macOS
Apple Silicon from `release_platform`. Fleet host addresses are never embedded
in the command implementation.

Installation reads `GITHUB_TOKEN.value` through Stado's admin-scoped
Skarbiec coordinates and exchanges it for a short-lived organization runner
token. That token travels only on the host channel's stdin and is consumed by
`config.sh`; it is not an argument, repository file, or persistent host
credential. The Actions Runner archive version and SHA-256 are pinned in the
Rust release.

The runner account is `stado-precheck`, with only `_work` and `_diag` writable.
A root-owned job hook removes prior workspace contents before and after each
job. Linux applies an nftables rule to that UID; macOS applies a PF rule to the
same account. The blocked CIDRs are the fixed RFC1918, loopback, link-local,
unique-local, and CGNAT network classes—not addresses of fleet hosts—so a job
can reach public GitHub/package services but not loopback, LAN, Tailscale, or
other private services.

### `stado host inventory`

The three facts this command reports could not be read any other way. `host
exec`'s allowlist gives none of them, and must not be extended to: every
entry there is a compile-time argv of absolute paths with no
operator-supplied path in it, and all three of these need `$HOME`. Without
the command, reading them meant a raw `ssh user@ip '<inline script>'` with a
hardcoded address — which is exactly what this replaces, repeatably and
through the registry-authorized channel.

It takes a registry target name and nothing else. There is no path, file
name, port or pattern to pass, because a command that accepted one would be
a command that could be pointed at `~/.ssh/id_ed25519`. Its remote program is
one compile-time script with no interpolation in it at all, run over the same
`deploy/host_channel.rs` every other read-only `host` command uses.

| Section | Contents |
|---|---|
| `managed_binaries` | `$HOME/.stado/bin/stado` and `$HOME/.stado/bin/skarbiec`: whether each exists, is a regular file, is executable, and what version it declares. A missing binary or a failed version call is an explicit `version_state` (`missing`, `not_executable`, `version_failed`, `version_empty`, `refused_symlink`, `refused_not_regular`), never a blank string. Symlinks are refused, not followed. Each row also carries `declared_version` — what the registry requires of this host — and `version_verdict`. |
| `forwards` | Every `$HOME/.stado/forwards/*.url` marker with its one-line loopback URL. A marker that is a symlink or not a regular file is reported as refused rather than read. |
| `listeners` | Listening loopback TCP ports and the pid that owns each, from `netstat -anv -p tcp`. |
| `subcommands` | Whether the installed `stado` knows each of a fixed, in-code list of subcommand paths, decided from the exit code of `SUBCOMMAND --help`. The subcommand itself is never run. This is version-skew detection. |
| `vaults` | The ACTIVE Skarbiec vaults: exactly `$HOME/.stado/*.vault.json`. Per file, metadata only — `name`, `state` (`regular`, `refused_symlink`, `refused_not_regular`), `bytes`, `mode` in octal, and `owner_only`. Symlinks are refused, not followed: the size and mode reported belong to the link, never to what it points at. |
| `vault_sidecars` | Everything else matching `$HOME/.stado/*.vault*.json`: snapshots, pre-migration copies such as `weles.vault.pre-v2.json`, and `*.acquisitions.json`. Same five fields, same refusals. |
| `reconciliation` | The answer, on three axes: per marker `matched`, `stale` or `unreadable` against the socket table; per marker `matched`, `disagrees` or `undeclared` against the registry; per binary `matched`, `behind`, `ahead`, `mismatched`, `undeclared` or `unknown` against `managed_versions`. Plus counts and the names behind each finding, and for vaults `vaults_not_owner_only` and `vaults_refused`. |

`vaults` and `vault_sidecars` are separate sections because the distinction
is operational, not tidiness. The active vault is state; a sidecar is
history. An operator who cannot tell them apart edits the wrong file, and
the file they meant to leave alone is the one holding live secrets.

Both sections are capped at 64 files each. Going over the cap is stated, not
silently swallowed: `vaults_seen` and `vault_sidecars_seen` carry how many
files actually matched, and `vaults_truncated` / `vault_sidecars_truncated`
say outright that the list above them is short.

Two vault findings are lifted into `reconciliation` because they are
conclusions, not table rows, and they print in the human-readable output as
well as under `--json`:

- **`vaults_not_owner_only`** — vaults whose group or other permission bits
  are set. A vault the group can read is an incident, not a cosmetic
  detail. Only regular files are judged here; a symlink is `lrwxrwxrwx` by
  construction, so listing one would report the link's permissions as a
  vault's and bury the real finding.
- **`vaults_refused`** — vaults refused as a symlink or as a non-regular
  file.

Neither turns `status` into a failure, for the same reason a stale marker
does not: the inventory reports drift, it does not punish it. A host with
every vault clean says so explicitly rather than printing nothing.

Reconciliation is the reason the command exists, and it runs on two
INDEPENDENT axes. They answer different questions, and a marker can pass one
while failing the other:

- **Marker against the socket table** (`reconciliation` on each marker,
  `stale_markers` in the summary). On `control-host` the marker
  `stado-weles-api.url` said `http://127.0.0.1:8766` while the admission API
  was listening on `8794`, and nothing in the fleet noticed, because nothing
  in the fleet read the markers. `stale` is that divergence, named.
- **Marker against the registry** (`declared_url` and
  `declaration_verdict` on each marker, `disagreeing_markers` and
  `undeclared_markers` in the summary). The marker is compared with the
  endpoint `service_directory` declares for THIS host — `endpoints[target]`,
  not the active host's endpoint, because a host standing by for a service
  still carries a declared endpoint for it. On `control-host` the marker
  `skarbiec-weles` says `8895`, something IS listening on `8895`, and the
  registry declares `19095`. The first axis calls that marker `matched`; the
  second calls it `disagrees`. That combination is the dangerous one:
  nothing is down, so no health check fires, and consumers resolving through
  the directory arrive somewhere else entirely. When both sides are loopback
  endpoints the port is compared, so `http://localhost:8895` and
  `http://127.0.0.1:8895` are one endpoint rather than a spelling dispute.

A third comparison runs over the binaries: `managed_versions` on the
registry target is the DECLARED version of each stado-managed binary, and
`version_verdict` is the host measured against it. `behind` and `ahead` are
decided numerically when both sides are three dot-separated numbers —
`0.4.392` is newer than `0.4.5`, which a text sort gets backwards — and by
exact equality otherwise, which yields `mismatched` rather than an invented
ordering. A target that declares nothing reports `undeclared`, never
`matched`: an unverified host must not read as a verified one. The number is
taken out of each binary's own answer shape by name — `stado --version`
prints `stado 0.5.1`, `skarbiec version` prints JSON whose `version` member
the remote script has already extracted — and an unfamiliar banner is
carried through whole rather than guessed at.

All three findings print in the human-readable output as explicit
conclusion lines, not as columns to interpret, and a host on which
everything agrees prints one line saying so.

Detection comes first on purpose. Nothing here deploys, restarts or
installs anything: a fleet that automates delivery before it can see the
difference between declared and actual state is a fleet with a faster way to
break production. `managed_versions` is the declaration, `host inventory` is
the visibility, and delivery is a separate command that has both to work
from.

Drift is reported, not punished: `status` stays `inventory` whenever the
inventory was collected, because a forward that was deliberately torn down
is not a broken host. Read `reconciliation.stale_markers`,
`reconciliation.disagreeing_markers` and `reconciliation.versions_behind`
for the verdicts.

What it deliberately does not show, and why:

- **No `lsof`, no `pgrep -f`, no process argv, no process environment.**
  That is where tokens, passwords and vault paths live. Listener ownership
  comes from the kernel socket table, which is why the `netstat -anv -p tcp`
  entry is already in `host exec`'s allowlist. Owners are bare pids; map one
  to a program with
  `stado host exec TARGET -- ps ax -o pid -o ppid -o etime -o comm`, or to a
  login user with `stado host exec TARGET -- ps ax -o user -o pid -o comm`.
  Both spell `-o comm`, the executable's name. `-o command` — the full argv
  — is deliberately absent from the allowlist and unreachable through it,
  because entries match exactly and the operator's words never join the
  command line. When `subcommands` comes back `probe_failed`, the follow-up
  question is `stado host exec TARGET -- sysctl -n kern.maxproc
  kern.maxprocperuid`: a host out of process slots is not a host running an
  old `stado`.
- **No file contents beyond the marker URLs**, which are non-secret loopback
  addresses. Every value it does read is reduced to a JSON-inert character
  set and capped in length on both the host and the control plane, so a
  corrupt or hostile file under `~/.stado` cannot push arbitrary text into an
  operator's terminal.
- **No vault contents, ever.** The vault sections report that a file exists,
  how large it is, its mode and whether anyone but its owner can read it.
  The remote script never opens a vault: not to read a byte of ciphertext,
  not to count items, not to name a consumer, not to check that the JSON
  parses. This is a boundary, not an oversight. `stat(2)` answers "which
  Skarbiec vaults are on this host" completely, so nothing in this command
  needs `open(2)`, and a diagnostic that reads secret files is a diagnostic
  that copies them into terminals, scrollback and CI logs every time
  somebody runs it. There is no flag to turn this off, because the field
  that would carry the content does not exist in the report shape. Read a
  vault with `skarbiec`, on purpose, not as a side effect of asking what is
  installed.

### `stado host software`

`host inventory` lists what is in `$HOME/.stado/bin`. It cannot say whether any
of it came out of a release, and on 2026-08-18 that was the gap that cost the
fleet a day: two macs were running a skarbiec built on a laptop — 0.2.1 on one,
0.2.3 on the other, neither in any published release, while `managed_versions`
declared 0.1.3 on both — and the older one was stripping the
`brama:agent:<id>` tags off a live credential every rotation. Nothing on any
screen could name the program doing it.

This command is the host stating what it runs. One row per program:

```
PROVENANCE  NAME      VERSION  SHA256        PATH
unmanaged   skarbiec  0.2.4    2e059a3abd19  /Users/charles/.stado/bin/skarbiec
release     stado     0.6.0    9d582e6e96da  /Users/charles/.stado/bin/stado
```

**`provenance` is decided by digest, on the host, and by nothing else.**
`host release` stages every delivery under
`$HOME/.stado/releases/<binary>/<version>/<platform>/` out of an archive whose
SHA-256 it verified against the canonical release manifest, then hard-links that
staged file into place — so the active file is byte-identical to a verified
published artefact. The reporter hashes each program and looks for that digest
among the staged artefacts: a match is `release`, no match is `unmanaged`. A
name, a version string and a program's own claim about its provenance all survive
one `scp`; a digest that equals a verified archive's extracted member does not.
A build delivered by `host install-binary` therefore reads `unmanaged` and that
is correct — it stages nothing under `.stado/releases`, and "the fix is running
but it did not come through the channel" is the finding, not a defect.

Three sources make up the population, and all three are needed. Every program in
`$HOME/.stado/bin` is what Stado placed. Every declared service unit's program is
what the host actually runs, which is not the same set. Every release-control
product install path is the third: brama lives at
`/Users/charles/.stado/services/brama/bin/brama` and appears in neither of the
others. The unit files and product paths are bound by the control plane from the
registry, so the host is asked to read files and hash bytes and is never asked
which of its files matter.

Shell scripts are counted, not rowed. control-host carries 1393 of them in
`$HOME/.stado/bin` against 28 programs — the retired helper channel had a writer
and no reaper — and a release pipeline produces none of them, so rowing each as
`unmanaged` would bury the twenty-eight answers the report exists to give. The
shebang decides, and it is tested before the executable bit: not one of those
1393 leftovers is executable any more, so filtering on the exec bit first made
every one of them vanish instead of being counted. The count is printed, so the
accretion stays visible.

The report is an **observation**, not a declaration: it is stored in
`~/.stado/observations.json` under `software:<name>@<host>` with the target as the
vantage, so it carries an age and goes stale exactly as a service reachability
observation does. That is what lets `stado release status` gate on it without one
ssh connection per target, and what makes "nobody has looked for three hours" a
visible state rather than a silent one. A read that fails is recorded as
`unverified` rather than swallowed, because leaving yesterday's report in place
after a refused connection is how an hour-old answer keeps reading as current.

```bash
# Take the report from one host and persist it.
stado host software control-host --json

# Read what every host has already reported, with ages.
stado host software
```

A host that has never reported is absent from the second listing and is a
**failure** wherever the report is judged — never a pass. See
"`stado release status` fails on silence".

### `stado host release`

The command the section above ends by pointing at, and the only thing in the
pack that owns "get this build onto that host". `managed_versions` is the
declaration of WHICH version, `stado-rs/data/products.json` is the
declaration of WHAT each product is, `host inventory` is the visibility, and
this is the delivery. It does not decide anything: `host inventory` says a
host is behind, and this closes the gap by carrying out exactly what the two
declarations already say.

```
stado host release TARGET --binary NAME --version X.Y.Z [--dry-run] [--json]
```

`--binary` names a declared product, and there is no hardcoded list of them
anywhere: `stado`, `skarbiec` and `weles-worker` are three entries in
`stado-rs/data/products.json`, which ships inside the binary that performs the
delivery. One entry names the artefact source (the `stado://releases/<product>`
segment and the exact archive member), the platform keys it is published for,
the install root on the host, the owning unit label when one exists, and how
the installed version is read back. Every field is required; a declaration
that omits one is refused when it is first read, so a half-declaration fails
every delivery identically instead of the ones whose code path happens to
look. A product publishes for the platforms it declares — `weles-worker`
publishes `darwin-arm64` only, and a delivery to a Linux host is refused on
the control plane rather than by a 404 on the box.

Two install shapes, because the fleet has two:

- **program** (`stado`, `skarbiec`) — one executable member, installed at
  `$HOME/.stado/bin/<name>` by one `rename(2)`, version read back by running
  it (`--version` in one plain line, or a `version` subcommand printing a JSON
  object).
- **tree** (`weles-worker`) — the install root IS the artefact directory
  (`$HOME/weles`). The declared payload member is unpacked into a versioned
  staging tree, and activation replaces every path the verified artefact
  carries, one rename each, retiring the path it replaces. The declared
  host-local paths — `recordings`, `var`, `.work`, state no release produced —
  are never named as a destination, never moved, and never removed; an
  artefact that carries one of them is refused at staging AND again at
  activation. The version is read back from a declared file inside the tree
  (`package.json` `/version`, the field the release itself is numbered from),
  and the delivered tree is asked again after activation.

The order of operations is the design, and it is Weles's shipped auto-deploy
order (`weles/scripts/worker/deploy/README.md`) applied to one binary:

1. **probe** — read the host: its platform, the version the installed binary
   declares, whether the coordinate is already staged. Writes nothing.
2. **stage** — read `release-manifest-<platform>.json` through the canonical
   Stado API, fetch the adjacent product archive, verify its SHA-256, extract
   the declared archive member, confirm the artefact declares the requested
   version, and publish it into
   `$HOME/.stado/releases/<product>/<version>/<platform>/`.
3. **activate** — re-check the staged version, then either hard-link the
   program beside the live one and `rename(2)` it over
   `$HOME/.stado/bin/<binary>`, or replace the tree's code path by path under
   its install root while the preserved paths stay exactly where they are.
4. **restart** — restart the unit the registry declares runs it, through the
   same program `stado service restart` uses.

The three remote phases are three separate programs on the shared channel,
not one script with three sections. That is what makes the guarantee
structural: the activate program is only ever sent after the stage program
reported a verified artifact, so a failed fetch, a mismatched digest or a
staged file that declares the wrong version all stop with the running
version untouched. `active_version_unchanged: true` says so in the report.

| Refusal | Why |
|---|---|
| `--binary` names no declared product | The operator's word selects a declared entry and never becomes a path or a URI segment — `host exec`'s rule. A refusal prints every deliverable product and what runs it. |
| The product declares no artefact for this platform | Publication is per product: a coordinate nobody published cannot be fetched, and saying so costs no ssh connection. |
| `--version` is not an exact semantic version | A coordinate is immutable. `latest` is a legal path segment, which is exactly why nothing here resolves an alias, a channel or a range. `+build` is refused too: it is not a canonical coordinate segment. |
| Missing or mismatched `release_platform` | Enrollment records the platform and inventory confirms it from the remote kernel before delivery. |
| Missing, malformed, or mismatched release manifest | The canonical catalog is the only digest source; delivery fails closed. |
| The registry declares no version for this host and binary | Delivery carries out a declaration; it does not stand in for one. |
| `--version` disagrees with the declaration | Change the declaration if that is the intent. Delivering past it would make the registry describe a host it no longer describes. |
| The canonical Stado API origin is not HTTPS | Checked here and again on the host. |
| The host's field sanitizer failed its own probe | Every string the host reported is then suspect, including the version this command would compare against. |

The digest is the archive `sha256` in the immutable canonical manifest. There
is no operator-local digest table and no release-specific API origin that can
drift from the Stado release catalog.

Running it twice is not running it twice: when the host already declares the
requested version the command reports `already_active` and sends no further
program — it does not re-fetch and call that a deployment. `--dry-run` runs
the read-only probe and nothing else, so "planned, not applied" is a
property of which programs were sent rather than a flag a longer script
promises to honour.

What it deliberately does not do:

- **No build, clone, tag lookup, package manager or channel pointer.** Weles's
  auto-deploy does none of those either. A host-side build is a host-side
  toolchain to keep alive, and a channel is a mutable coordinate.
- **No version choice.** It never picks "the newest" and never writes the
  registry. Deciding what a host should run is upstream of putting it there.
- **No rollback**, because there is nothing to roll back from: every failure
  happens before activation. Rolling back is `host release` naming the
  previous version, which is why the versioned staging tree is kept.
- **No invented unit.** A product declares a unit label alone, which must be
  FOUND in the registry's declared service set before anything restarts it, or
  a label together with the unit file that runs it, which is itself the
  statement that the unit exists. A product with no declared unit —
  `skarbiec`, a CLI rather than a daemon — is activated and reported as having
  no unit, never as "restarted".
- **No host-local state in a delivery.** A tree delivery replaces code. The
  preserved paths are declared, printed by `--dry-run`, and left untouched.
- **No symlink at `$HOME/.stado/bin/<binary>`.** The active path stays a
  regular file, hard-linked to the staged inode, because `host inventory`
  refuses to read through a symlink and would otherwise report the active
  binary as unreadable.

### `stado host gates` and `stado host reclaim`

One incident, two commands. The Mac mini's data volume sat at roughly 2 GiB
free against a registry policy that wants 55 GiB. The queue agent computes
`disk_pressure_unresolved` every tick, publishes that word in its capacity
broadcast, and fails admission CLOSED while it is true — zero free slots, no
claim, deliberately. So the host claimed nothing for hours, every release build
queued behind it, the Brama candidate could not even start, and no command in
this CLI said any of it out loud: `host disk` printed the free bytes and the
policy but never the admission verdict, `registry doctor` listed the host as
broadcasting normally, and the fact that mattered lived only in
`capacity/<consumer>.json`, which nothing read. The space eventually came back
by hand, over ssh, from a script written during the outage.

`gates` is the read half. It joins three sources and re-derives none of them:
the host's own capacity publication, whose `diag` words are reported verbatim
so a blocker an operator reads here is greppable in the agent that published
it; the registry target's declared `slots` and `disk_cleanup` policy; and
`df -Pk /` plus the janitor's state file, read with the exact script `host
disk` sends, so the two commands cannot disagree about how much space a host
has. `claiming` is false when any blocker is present, and the exit status
follows it, the way `host ping`'s follows its combined verdict.

```bash
stado host gates control-host
```

```
host:     control-host
claiming: no
blockers: disk_pressure_unresolved
disk:     2 GiB free, low watermark 55 GiB, target 80 GiB, policy enforce
capacity: 0 free slot(s) of 2 declared, published 24s ago (2026-08-18T09:14:02+00:00)
```

Busy slots are deliberately NOT a blocker: a host running work claims nothing
more and is perfectly healthy, and calling that blocked would make the command
cry wolf on every loaded box. A publication that is missing entirely, or older
than the staleness horizon every live-capacity reader in the fleet filters on,
IS a blocker — the scheduler cannot see such a host at all. A stale row is
still reported, with its age, because "the agent said this an hour ago" and
"nobody ever said anything" send an operator to different places; its verdict
is recomputed from the numbers the command just measured rather than trusted.

`reclaim` is the write half, and it previews by default. Four stages run in
this order, and nothing else runs:

1. `registry_cleanup` — the host's OWN janitor, invoked exactly the way `host
   cleanup --dry-run` invokes it, so the policy stays the one the registry
   declares and the command contains no cleanup policy of its own. The item
   count is the janitor's: eligible items in a preview, deleted items in an
   apply.
2. `build_scratch` — `$HOME/.stado/build-work`, the release build scratch tree
   the checked-in build helpers work in and never clean up.
3. `delivered_trees` — the version directories under `$HOME/.stado/services`,
   where every `service deploy` and every artifact install stages one tree per
   version and keeps the previous one beside it as `current.before-<version>`
   so a rollback is a rename; **and** every superseded delivery root the
   product catalog declares (`superseded_roots` in
   `stado-rs/data/products.json`). The mini carries 20 `weles-worker` versions
   — 0.5.2 through 0.5.21, 9.7 GiB — under `$HOME/.local/share/weles-worker`,
   staged by the installer that predates the artifact install path, while the
   worker itself runs from its own checkout: trees no rollback will ever reach
   and, until that root was declared, trees no command could even report. The
   roots come from declarations so the next time a delivery path moves it is a
   data change; a product's LIVE install root is never swept, because for a
   `tree` product that root IS the running installation.
4. `chromium_clones` — `org.chromium.Chromium.code_sign_clone` under this
   account's macOS temporary container. macOS clones the whole browser bundle
   on EVERY launch so it can validate a signature against an object nobody can
   swap underneath it; Weles drives Chromium for browser automation, and a run
   that is killed leaves its clone behind. On the mini the day this landed: 137
   clones, 130 of them untouched for more than a day, and until then neither
   the janitor nor any command removed or reported one of them. The container's
   name carries a per-account hash, so it is the OS's own answer (`$TMPDIR`,
   with `getconf DARWIN_USER_TEMP_DIR` behind it) and the stage refuses any
   value that is not under `/var/folders`. Only entries macOS itself named
   (`code_sign_clone.*`) are candidates, and the newest clone in the root is
   kept whatever its age: macOS records nothing about which process owns which
   clone, so a browser that has been up longer than the age gate is exactly the
   owner of the most recent one.

The same eviction lives in the janitor as the `chromium_clones` cleaner, so a
host whose registry policy declares it reclaims these on its own interval —
same age gate, same live-process snapshot, same newest-clone rule, plus the
janitor's per-pass byte and item caps — and `host disk` and `host cleanup
--dry-run` report its counts beside the other cleaners. The registry floors its
`min_age_seconds` at a day. The stage is for the hosts and the moments where
the policy has not declared it.

Four rules are encoded in the command rather than left to whoever is at the
keyboard. Nothing outside those declared roots is touched: every candidate is
produced by globbing or `find`-ing one of them, and no path arrives from the
registry or from the operator — the one value that comes from outside is the
macOS temporary container, which the OS itself reports and which is refused
unless it sits under `/var/folders`. Nothing a live process holds is removed:
one `ps` snapshot is taken before any stage and every candidate is checked
against it, taken once into a variable because `ps | grep <path>` matches the
grep's own argv and would report every candidate as held. The newest tree of a
product, the newest clone in a root, and whatever `current` resolves to are
always kept, even when they are the largest thing there, and nothing younger
than a day is a candidate at all — which is what makes the stages safe against
a delivery or a browser session that is in flight, since its directory is both
the newest and the youngest. And the same program runs in both modes with the
removal itself behind the mode flag, so a preview walks exactly the paths an
apply would take rather than a second implementation's guess at them.

```bash
stado host reclaim control-host
stado host reclaim control-host --apply \
  --reason 'queue agent has published disk_pressure_unresolved since 08:10'
```

```
DRY RUN — nothing on control-host is deleted. Re-run with --apply --reason <text> to remove what follows.
STAGE             FREE BEFORE  FREE AFTER  ITEMS
registry_cleanup  2 GiB        2 GiB       7
build_scratch     2 GiB        2 GiB       3
delivered_trees   2 GiB        2 GiB       98
chromium_clones   2 GiB        2 GiB       130
  build_scratch /Users/charles/.stado/build-work/stado
  delivered_trees /Users/charles/.stado/services/weles-worker/0.4.9
  chromium_clones /var/folders/zy/l0_0w9dn0k94n1b7xnt7kpv80000gn/X/org.chromium.Chromium.code_sign_clone/code_sign_clone.lovzyd

free: 2 GiB -> 2 GiB
```

`--apply` is the only thing that deletes, and it refuses to run without
`--reason`: the owner-only record it appends to
`$HOME/.stado/audit/host-reclaim.jsonl` on the host is the only account of why
several tens of gigabytes left that machine. It carries the reason verbatim, who
ran it — `service ensure`'s spelling of that, not a second one — and what each
stage did. The record is written after the stages, because the measurements have
to exist before it can be true, and it lives on the machine whose disk changed
rather than in a central ledger — the operator who reclaimed the space may
never touch this control plane again, and a record kept anywhere else is a
record that can be missing exactly when someone asks what happened to that box.
A stage the host could not run at all is reported under its own name with the
`_unavailable` suffix and null measurements, never as a stage that freed
nothing.

What no stage can give back is reported rather than left as a hole in the
arithmetic: `host disk` names the host's local APFS snapshots, and `host gates`
adds a `local_snapshots_unreclaimable` NOTE — never a blocker, so it cannot
change `claiming` or the exit status — while the disk is the reason a host is
claiming nothing. Their blocks are inside the `used` figure `df` reports, no
stado command removes them (dropping a snapshot is dropping a restore point,
which is an operator's decision about that machine's recovery), and macOS
publishes no size for one: `tmutil`, `diskutil apfs listSnapshots` and
`diskutil info` each name them and none of them measures them, so the count and
the host's own snapshot names are reported and no byte figure is invented from
them. `tmutil thinlocalsnapshots` is the tool that reclaims them, and it stays
in the operator's hands.

```
local APFS snapshots: 3 — their blocks are inside USED above, no stado command removes them, and macOS reports no size for them. Thin them with tmutil if the space is needed:
  com.apple.os.update-DEDECEC55622993FB7EF1CB6A97E976433E7F84ECCE5514C9C380AF59534732D
  com.apple.os.update-FE6B60577C481C0803254FA3E9B1ED789ECB0A02B601EFA2F42A756BCACCF7B59887777727FAC21DD5EAE2AFC3E2C69F
  com.apple.os.update-MSUPrepareUpdate
```

## `stado fleet`

Enrollment and the SSH channel it rides. Four methods add a machine — `invite`,
`adopt`, `join` and `declare` — and `stado fleet methods` is the command that
names them, says what each requires and provides, and reports whether the
registry's enrollment catalog allows it. Whichever method is used, the registry
is written only after the machine has been read: `enroll` probes the target over
the stored target-scoped key and records the hostname and platform it observed,
and `approve` does the same on the destination the machine reported, so a
machine that cannot be reached or cannot take the agent does not stay
registered. In every method the private half of the channel key stays in the
operator's credential store and only the public line reaches the machine; no
method transmits a private key or asks the machine to generate a pair.

| Subcommand | Behavior |
|---|---|
| `stado fleet key generate TARGET` | Generate a fresh ed25519 pair for the target into the selected credential store and print the public half — the line that goes into the machine's `authorized_keys`. The private half is never printed. Leaves the stored key readable to the local operator, so no separate grant step is needed. |
| `stado fleet key install TARGET` | Append the stored public key to the target's `authorized_keys` **through the existing channel**. A rotation tool, not first contact. |
| `stado fleet key check TARGET` | Verify the stored key actually opens the channel to the target. |
| `stado fleet key rotate TARGET` | Rotate the target's key end to end, with rollback on failure. |
| `stado fleet key ls` | List stored SSH keys as metadata only. |
| `stado fleet key rm TARGET` | Remove a target's SSH key from the credential store. |
| `stado fleet methods [--json]` | The four ways to add a machine — `invite`, `adopt`, `join`, `declare` — each with the command that performs it, what it requires, what it provides, whether the registry allows it, and the catalog field that gates it. `--json` emits `{"methods":[{"name","command","summary","requires","provides","allowed","gate"}]}` in that fixed order, with `gate` naming a registry field such as `registry.enrollment.allow_invite` (`null` for `declare`, which no field gates). A method the catalog disables is still listed, marked disabled. |
| `stado fleet enroll NAME --ssh DEST [--install-key] [--kind local] [--fleet NAME] [--bootstrap]` | Probe-then-write onboarding: reads `hostname`, `uname -s` and `uname -m` over the channel, writes the entry from what it read, optionally assigns a fleet, and with `--bootstrap` installs the agent and rolls the entry back if that install fails. `--install-key` is the `adopt` method: before probing, mint the target's pair if needed and append its **public** line to the machine's `authorized_keys` over the access plain `ssh DEST` already has (agent, an operator key, or OpenSSH's own password prompt). Idempotent, and never connected / authentication rejected / write failed are reported apart, all before any registry write. |
| `stado fleet invite [--name NAME] [--offline] [--expires 24h] [--uses 1] [--json]` | The `invite` method, in two modes, because the one-line form can only work where the machine being added actually reaches the control point. Common to both: the channel key `stado-ssh-NAME` is minted first through the `fleet key generate` path and its fingerprint printed, the private half never leaves the store, a name already held by a target or by another open invite is an error rather than a suffix, `--expires` takes an integer plus `s`, `m`, `h` or `d` and refuses a bare number, `--uses 0` is an error, and a recording failure removes the freshly minted key again. Without `--offline` the command takes the control address from `enrollment.url`, else the live entrance published by [`stado fleet ingress`](#stado-fleet-ingress), else `api.url`, runs [the control-point check](#the-control-point-check) against it, and prints the one-liner only if `/join.sh` answered `200`; when the address came from the ingress it says so and warns that a quick-tunnel address is temporary and changes on restart. `--offline` consults no ingress, since it probes nothing. **Online mode:** mint `<id>.<secret>`, print it once — nothing can reprint it — store `secret_sha256` and nothing else, and print `curl -fsSL <control-point>/join.sh \| sh -s -- <id>.<secret>` with the resolved host, never a host compiled into Stado. **Offline mode** (`--offline`, or any failed check): no token is minted, so there is nothing to intercept, replay or lose. The command prints a paste-ready `sh` fragment between two markers which, run by the machine's owner, creates `~/.ssh` at 700 and `authorized_keys` at 600, appends the fleet's **public** line there idempotently — the line is carried inside the fragment and fetched from nothing — reports whether anything answers on port 22 and names the exact macOS Settings path or Linux equivalent when nothing does, and prints as its last line the `user@address` the owner sends back; the fragment states in its own output that it carries only a public key and is therefore not a secret. The stored invite records `mode: "offline"`, `status: "open"` and no `secret_sha256` at all, and `stado fleet enroll NAME --ssh ADDRESS --bootstrap` closes it as `spent`. `--json` always carries `id`, `mode`, `target_name`, `created_at`, `expires_at`, `uses_allowed`, `public_key`, `authorized_keys_line`, the `checkpoint` object (`url`, `probed`, `reachable`, `reason`, `detail`), `base_source` (`enrollment.url`, `ingress` or `api.url`) and `base_is_temporary`, plus `base_warning` when the base is a tunnel; online adds `token`, `token_shown_once: true` and `join_command`, so redirecting that output to a file writes a live credential to disk and nothing can reprint the token if it is lost; offline adds `snippet`, `snippet_is_not_a_secret: true` and `next_step`, none of which is a credential. |
| `stado fleet invites [--json]` | Every invite and the state it is actually in: id, target name, status (`open`, `spent`, `revoked`, `expired`), uses spent of uses allowed, timestamps, who created it. Never the token or the secret. An open offline invite reads `open (offline, awaiting address)` — the one state here that waits on a person rather than on a clock — and its other states are marked `<status> (offline)`; online invites are unchanged. `--json` emits `{"invites":[…]}`, each row carrying `mode` and `awaiting_address` alongside the fields above. |
| `stado fleet revoke-invite ID` | Close one invite immediately, by id. A revoked token is refused exactly like a spent, expired or unknown one. |
| `stado fleet ingress up [--port N] [--named]` | Stand up the public entrance the one-line invite mode needs: a `stado dashboard --enrollment-only` listener on a free loopback port behind a Cloudflare quick tunnel, with no Cloudflare account, API token or DNS record. Publishes `enrollments/ingress.json` only after fetching `/join.sh` back through the public address from the internet and matching it against the script this build serves; any failure before that stops both processes and names the stage. A `--port` already in use is refused before anything starts. `--named` is refused: the vault has no `platform-admin-cloudflare#api_token` field. See [`stado fleet ingress`](#stado-fleet-ingress). |
| `stado fleet ingress status [--json]` | What is published, whether that address answers now, when it was last verified from the internet, how long it has stood, which loopback port the listener holds, and whether both processes are alive. |
| `stado fleet ingress down` | Close the tunnel, stop the listener, and remove `enrollments/ingress.json`. Every one-liner minted against that address stops working. |
| `stado fleet join` | The `join` method, run **on the machine being added**: announce it to the fleet when the control plane cannot reach it but it can reach the store. |
| `stado fleet pending [--json]` | List unanswered join requests. An invited request also shows the target name the invite reserved, the SSH destination approval will probe, the invite id it came from, the fingerprint of the key the machine installed, and whether that machine's SSH channel was answering when it reported — an approval spent on a machine with Remote Login still off is a wasted round trip. `--json` emits `{"pending":[{"hostname","os","arch","kind","status","requested_at","target_name","destination","invite_id","installed_key_fingerprint","ssh_listening"}]}`; the invited-only keys are `null` for a plain `join` request, so `select(.destination != null)` is what isolates invited ones. |
| `stado fleet approve HOSTNAME [--fleet NAME]` | Turn a pending join request into a registered target, over the destination the request carries, through the same probe-then-write enrollment — approval does not skip the probe. The argument is the request's hostname; an invited machine is registered under the target name its invite reserved (which is the name whose key the invite minted), and its probed hostname lands in the entry's `hostnames`. |
| `stado fleet reject HOSTNAME` | Drop a pending join request. |
| `stado fleet catalog [--json]` | Print the registry's central enrollment and communication catalog, which every enrollment path honours: `allow_invite`, `allow_adopt`, `allow_join`, `allow_enroll`, `require_verified_hostname`, `key_custody`, and the `channels` block. All four `allow_*` default to `true`, including when an `enrollment` section exists but omits them. |
| `stado fleet list [--json]` | The fleets declared in the registry with their members. |
| `stado fleet status NAME` | Live state for the members of one declared fleet. |
| `stado fleet create NAME [--notes TEXT]` | Declare a new fleet in the canonical registry. |
| `stado fleet assign TARGET FLEET` | Add a registered machine to a declared fleet. |
| `stado fleet doctor [--json] [--fleet NAME]` | Worker health: agent grant, secret probes, beacons, capacity. |

The channel is the same one the `stado host` commands use, and the SSH
destination is whatever the operator supplied: a `.local` name on the local
network is as valid as a tailnet name, and the registry requires no particular
kind. A `.local` destination limits every channel-opening command to that
network; it does not limit `stado registry beacon-age` or `stado host health`,
because the host publishes its beacon outward.

Stado Desktop reaches this family through the dashboard's authenticated
operator-command bridge instead of carrying its own enrollment logic, so its
**Fleet › Hosts › Add a Machine** chooser offers the same four methods and
performs exactly the commands above. The separate `stado_fleet` binary remains
for compatibility over the same implementation; `stado fleet` is the documented
surface.

### The control-point check

`stado fleet invite` prints a `curl` one-liner only when it has proven that the
line can work, because a one-liner naming an unreachable host is worse than no
one-liner: it moves the failure onto somebody else's machine, hours later, with
no way to tell a typo from a fleet that never published an ingress. The check
works out the control address from three sources, in order — `STADO_ENROLLMENT_URL`
/ `enrollment.url`, then the verified entrance `stado fleet ingress` published
in `enrollments/ingress.json` (used only while it still answers), then
`STADO_API_URL` / `api.url` — fetches `/join.sh` from it, and requires `200`. No
control host is built into Stado; there is nothing to fall back to silently.

| Reason | What it means |
|---|---|
| `not_configured` | No control address is configured at all. Not an error: an unconfigured fleet has no published control point, which is the normal state of a loopback-bound dashboard. |
| `name_does_not_resolve` | The configured host is in no zone the resolver can answer for, so nothing anywhere can fetch `/join.sh` from it. |
| `connection_refused` | The name resolves, but nothing accepted the connection (refused, or timed out). Typically a dashboard still bound to loopback, reached from off-host. |
| `route_unknown` | Something answered, with a status other than `200`. The release serving that host predates the invite routes, or the proxy in front of it does not forward them. |
| `forced_offline` | `--offline` was given, so no probe ran. |

The success verdict is `ok`, and it is the only one that yields the one-liner.
Every one of the five in the table continues in offline mode and prints which
applied; the `curl` line is printed in none of them, and `--json` carries the
verdict as `checkpoint.reason` next to the sentence in `checkpoint.detail`.
`--json` also carries `base_source` (`enrollment.url`, `ingress` or `api.url`)
and `base_is_temporary`, and a one-liner built on an ingress address carries
`base_warning` as well — the text output says the same thing in prose. What it
takes to make the online mode reachable, and the one command that provides it
without any Cloudflare credential, is set out under
[what the one-line mode needs](onboarding.md#what-the-one-line-mode-needs-and-how-to-stand-it-up).

### `stado fleet ingress`

The public entrance the one-line mode needs, as a command rather than a runbook.
`up` chooses a free loopback port, starts `stado dashboard --enrollment-only` on
it, starts a Cloudflare **quick** tunnel in front of it — no account, no API
token, no zone, no DNS record — waits for the `*.trycloudflare.com` address that
tunnel prints, and then fetches `/join.sh` back through that address **from the
internet** and compares the bytes with the script this binary serves. Only a
match publishes `enrollments/ingress.json`. Every failure before that point
stops both processes and names the stage — `listener`, `tunnel`, `verification`
or `publication` — so no half-open entrance is ever left behind, and no operator
is ever told an address works because it probably does.

| Command | What it does |
|---|---|
| `stado fleet ingress up [--port N] [--named]` | Stand the entrance up and publish it once it has answered from the internet. The port is chosen automatically; `--port N` is bind-tested first and a port already in use is **refused before any process starts**, because a tunnel in front of a service this command did not open is the one mistake it must not make. `--named` is refused in one sentence: a named tunnel needs a Cloudflare API token, the vault has no `platform-admin-cloudflare#api_token` field, and Skarbiec refuses to grant on a field that does not exist. |
| `stado fleet ingress status [--json]` | Whether anything is published, whether that address answers **now**, when it was last verified from the internet, how long it has been standing, which loopback port the listener holds, and whether the two processes are still alive. `--json` emits `{"published","base_url","mode","host","started_at","verified_at","standing_seconds","seconds_since_verified","listener_port","reachable","reason","detail","processes_on_this_machine","listener_alive","tunnel_alive","pid_hint","temporary"}`; an unpublished ingress emits `{"published":false,"detail":…}`. |
| `stado fleet ingress down` | Close the tunnel, stop the listener, remove `enrollments/ingress.json`. Both are signalled as process **groups**, so nothing either of them spawned keeps the port. A pid whose command line no longer matches is reported as gone rather than signalled, and an object recorded by another machine is refused instead of acted on. |

The published object carries `base_url`, `mode` (`quick` or `named`), `host`,
`started_at`, `verified_at`, `listener_port` and `pid_hint` — the machine that
owns the processes, both process-group ids, and both log paths, which is exactly
what `down` and `status` need in order to find them without guessing. Logs live
in `$HOME/.stado/ingress/`. `cloudflared` is resolved the way Stado resolves
every external binary: `STADO_CLOUDFLARED_BIN` if set, then
`/opt/homebrew/bin/cloudflared`, `/usr/local/bin/cloudflared`, then `PATH`; a
miss names every place that was looked in.

Both processes outlive the command — an entrance that dies with the terminal
that opened it is not an entrance — and both are started as process-group
leaders, so a Ctrl-C aimed at a later command cannot take the fleet's front door
down with it.

Two steps inside `up` look like detail and are not.

**The tunnel presents the loopback authority, not the public name.** The
dashboard carries a DNS-rebinding guard that accepts a loopback `Host` and
answers `403` to a DNS one unless a reverse proxy has been explicitly trusted.
A tunnel forwarding `Host: <name>.trycloudflare.com` verbatim therefore gets
`403` on all three enrollment routes, so `up` starts `cloudflared` with
`--http-host-header 127.0.0.1:<port>` — the authority it is genuinely
connecting to, exactly as any reverse proxy in front of a loopback bind does.
The guard is not relaxed and nothing else on the machine becomes reachable.

**DNS is waited for through Cloudflare's own resolver, never through this
machine's.** The `*.trycloudflare.com` record appears a few seconds *after*
`cloudflared` prints the address, and a `getaddrinfo` issued in that window
does not merely fail — it leaves an `NXDOMAIN` in the local resolver's negative
cache. Measured on this fleet's operator machine: one lookup at second zero
made the address unresolvable for the next 64 seconds, while Cloudflare's own
resolver had been answering since second six; where the zone's negative TTL is
honoured rather than clamped that is 1800 seconds. So `up` asks Cloudflare's
DNS-over-HTTPS endpoint whether the name is published, and only then does
anything resolve it. A resolver it cannot reach is not treated as a failure —
the step protects the local cache, it does not decide anything.

Two properties of a quick tunnel are printed by `up` and by `status`, and they
are not incidental. **Cloudflare documents quick tunnels as non-production and
rate limits them**, which is an acceptable trade for an entrance used a handful
of times a month to add a machine and for nothing else. And **the address is new
on every start**: stopping and restarting the ingress invalidates every
one-liner already handed out, which is why there is no `restart` subcommand —
`down` then `up` makes what happened visible.

### Invite endpoints on the dashboard

The `invite` method needs three routes on the dashboard, because the machine
being added has no Stado binary, no store credential and no operator identity —
it has one token. All three are served by the same authenticated dashboard
process that serves `/api/object`, `/api/machine/*` and `/api/operator/run`, and
all three sit outside that surface's operator authorization: they accept **only**
an invite token, and **none of them can write the registry.** The registry write
happens later, in `stado fleet approve`, under operator authority. All three are
useful only where that dashboard is reachable **from the machine being added**,
which a loopback-bound dashboard is not: the routes are the online mode's
mechanism, and the offline mode exists because they cannot be assumed. They are
not a prerequisite for adding a machine.

| Route | Authorization | Effect |
|---|---|---|
| `GET /api/fleet/invite/key` | the invite token as bearer, nothing else | Returns `{"target_name","public_key","authorized_keys_line"}` — the public half only. Reads; writes nothing. |
| `POST /api/fleet/join` | the invite token as bearer, nothing else | Body `{"hostname","os","arch","destination","installed_key_fingerprint"}`, plus `ssh_listening` so the operator sees an unreachable channel before spending an approval on it. Writes the join request `enrollments/<hostname>.json` with status `pending`, the `invite_id` and the reported `destination`, and increments `uses_spent`. Creates no registry entry and cannot modify one. |
| `GET /join.sh` | none | The script the machine runs — [`deploy/join.sh`](../deploy/join.sh), embedded verbatim into the binary at build time, so the served script and the reviewed one cannot diverge. It discloses nothing: the secret is the argument its user supplies, and the script fetches the two routes above with it. It needs only a POSIX shell and base utilities — no Stado binary, no `jq`, no Python — installs the public key, reports in, and installs no software. |

A token that is spent, expired, revoked or simply unknown is refused the same
way on both API routes, without revealing which of those four states applies.
`stado fleet invites` is the operator-side view of the same objects, and
`stado fleet revoke-invite ID` is how a token stops working before its expiry.

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
| `stado registry host add HOST --ssh DEST --release-platform PLATFORM [--kind local]` | The `declare` method: onboard a machine into the registry, validated, refusing duplicates. `--ssh` and `--release-platform` are both required. Declaration only — it reads nothing from the machine, so `stado registry doctor` is what later diffs the assertion against reality, and `stado fleet methods` lists it alongside the three probing methods. |
| `stado registry beacon-age [--json]` | Every registry host and its last beacon, worst first. |

### Registry document shape

Beyond `schema_version`, `targets` and `coordinators`, the canonical
document carries two blocks the fleet resolves services with:

- **`service_directory`** — `authority` (the `target` allowed to publish and
  the `command` it publishes with), a monotonic `generation`, and `services`.
  Each service names its `active_host`, an `endpoints` map of `host -> url`
  (standby hosts included), a `consumers` map of `consumer -> capabilities`,
  and either the `placement_profile` that relocates it or the
  `managed_service` unit that owns it. The `active_host` entry of `endpoints`
  is the only endpoint a consumer may call: a host carrying an endpoint is
  not thereby serving.
- **`placement_profiles`** — service groups that move between hosts together:
  `services`, the separate `stop_order` and `start_order`, the `state` files
  that travel with them (`required` state missing aborts the move), a `hosts`
  map giving each host's launchd/systemd `units` and health `probes`, and
  `routing`.

**Unknown keys are preserved.** A registry write replaces the whole
document, so a writer that does not model a key used to delete it — on
2026-08-04 the canonical document lost `channels`, `enrollment` and `fleets`
that way. Every top-level key this build does not model (`inference` today)
round-trips verbatim through `Registry::extra`, `stado registry push` still
refuses a payload that removes a top-level key unless forced, and a
`service_directory` or `placement_profiles` block that is present and
malformed is an error rather than a silently empty one.

**`SERVICE_DIRECTORY_STALE`** is the code a consumer holding a directory
older than the published `generation` gets back, with the message naming
both generations and telling the caller to refresh (`stado registry pull`)
and retry. It is a distinct, retryable code precisely so a handed-over
service does not surface as a refused connection and send the operator to
the network instead of to the directory.

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

## `stado builds`

Native builds. A build recipe lives in the top-level `builds` key of the
canonical registry document and names a repository, the branch to watch, one
POSIX sh build command, the artifact paths the checkout leaves behind and the
release platforms it is built for. The control-plane poller
(`coordinator_loop`) checks each enabled recipe at its `interval_seconds`
cadence with `git ls-remote`; a branch head it has not seen enqueues one
ordinary queue job **per platform**, each cloning the branch shallow, running
the build command and uploading the artifacts under its own canonical results
URI (`status/<job_id>/output/`). Every mutation is the same fenced registry
read-modify-write `stado host declare-version` uses.

**Platforms.** `--platform` takes a published release platform word —
`darwin-arm64` or `linux-amd64` — and is repeatable and required. Each job
declares that platform as its `platform_os`/`architecture`, and a worker
refuses to claim a job whose platform is not its own, so a Linux build is
never built on a Mac because a Mac was free. The outcome is recorded per
platform under the recipe's `runs` map, keyed by the platform word: one
recipe, one row per platform, each with its own job, status and version. A
job carrying no platform fields is unconstrained, so every job submitted
before platform routing existed stays claimable everywhere.

**Versions come from tags, never from the poller.** After a build job
finishes, its run's `version` is the exact tag the built commit carries, with
a leading `v` stripped, and only when that tag is an exact semantic version
(`1.4.2`, `1.4.2-rc1`; build metadata is refused). The tag is resolved on the
build machine right after the clone — before the build command runs, so a
build that fetches or tags inside its checkout cannot change the answer — and
written into the uploaded output as `stado-build-version.txt`, empty for an
untagged commit. An untagged commit therefore produces artifacts and **no**
version, which is the normal case and not a failure. That file is the run's
own bookkeeping and is not listed among its `artifact_uris`.

**`--auto-declare`.** Off by default. When on, a run that succeeded *and* has
a version declares that version as the fleet's managed version of the
product the recipe's **name** selects, on every registry host whose
`release_platform` equals the run's platform — through the same code path
`stado host declare-version` runs, with one line per host. The run's
`declared` flag turns true only when every matching host took it. A run with
no version is skipped with the reason logged; a recipe whose name is not a
declared product declares nothing and says so. On every commit this would
move the fleet on every commit, which is why it is opt-in per recipe.

**Boundary: a build is not a release.** Builds publish artifacts and record
versions. They never write `release_control.products[...]` desired state.
Promoting a *signed* release re-fetches every platform's archive, verifies
the exact bytes, the signature and the passed qualification against a release
key, and only then moves desired state — that is `stado release promote`, and
it stays a deliberate, separate step. Collapsing the two would let a poller
publish an unverified build to the fleet. `stado service converge --apply`
still does the delivery.

**Editing a recipe.** `stado builds edit` takes the same flags as `add`,
every one optional: a flag not given leaves its field alone, and
`--artifact`/`--platform` given at all REPLACE the recorded list. The state
semantics decide whether the recipe re-fires. A changed `--repo` or
`--branch` is a *different source*, so the edit clears `last_seen_ref` and
every recorded run — the runs describe commits of the old source, and a
retained head would leave the new source's current head unbuilt until it
happened to move. A changed `--command`, artifact set, platform set,
interval or auto-declare flag says how the *same* source is built, so
`last_seen_ref` and the runs are kept: this head was already built, and the
poller only fires when it moves. An edit naming no flag at all is refused,
and an edit whose values already stand writes nothing and says so.
`enabled` is not editable here — `enable` and `disable` own it, because
whether a recipe builds is a decision, never a side effect of correcting a
build command.

| Subcommand | Behavior |
|---|---|
| `stado builds list [--json]` | Every recipe: source, enabled flag, auto-declare flag, last seen commit, then one run row per platform (platform, status, version, declared, when). `--json` emits the recipe array verbatim. |
| `stado builds add --name N --repo URL --branch B --command C --artifact PATH... --platform P... [--auto-declare] [--interval-seconds N] [--json]` | Add a recipe, validated (kebab-case name, `https://` repo, relative artifact paths, known platform words; a platform named twice is built once). Recipes start **disabled**: polling a repository is explicit opt-in, never a side effect of writing it down. `--json` emits the created recipe. |
| `stado builds edit NAME [--repo URL] [--branch B] [--command C] [--artifact PATH...] [--platform P...] [--auto-declare | --no-auto-declare] [--interval-seconds N] [--json]` | Change the named fields in place; absent flags leave their fields alone, lists given at all replace. Changing the source clears `last_seen_ref` and the runs; changing anything else keeps them. `--json` emits the updated recipe. |
| `stado builds remove NAME [--json]` | Delete the recipe. `--json` emits `{"name": ..., "removed": true}`. |
| `stado builds enable NAME [--json]` | Start polling the recipe. |
| `stado builds disable NAME [--json]` | Stop polling without deleting; `last_seen_ref` survives, so re-enabling does not rebuild a commit already built. |
| `stado builds run NAME [--json]` | Enqueue one build job per declared platform now, cadence and enable flag notwithstanding — how a recipe is vetted before it is enabled. |
| `stado builds status NAME [--json]` | The recipe plus, per platform, the recorded run and the live queue state of its job. |

A top-level `builds_disabled: true` in the registry halts all build polling
fleet-wide without touching any recipe's own flag. A build already submitted
still has its outcome recorded while the switch is set — a run stuck at
`running` forever would be the switch corrupting the record — but
`--auto-declare` is withheld, because acting on a build is exactly what the
switch takes away.

## `stado release`

| Subcommand | Behavior |
|---|---|
| `keygen --private-key PATH --public-key PATH --key-id ID` | Create an Ed25519 release authority. The private file is mode `0600`; only the public key belongs in registry trust policy. |
| `build --repo URL --version TAG --platform P [--host H]` | Produce the archive `prepare` signs. A builder is chosen from the registry by the platform it reports, the tag is checked out clean, and the product's own `.stado/release.json` says what to build and what belongs in the archive — Stado never learns how a particular product is assembled. The archive is brought back to the caller rather than signed on the builder, so the release authority's key never travels to a build host, and the builder avoids the host a service is placed on where another can do the work. |
| `prepare PRODUCT VERSION PLATFORM ...` | Hash an existing archive, bind source revision, schema compatibility and qualification evidence into a canonical manifest, sign it, then publish archive, signature and manifest create-only. The manifest is the last commit marker. |
| `promote PRODUCT VERSION --channel candidate|stable` | Re-fetch every platform, verify exact bytes, signature and passed qualification, then compare-and-swap one `desired` registry generation. It never rebuilds. |
| `agent --target TARGET [--once]` | Reconcile canonical desired state on a host: verify, stage immutably, start a private candidate, check readiness, switch the stable proxy, drain, monitor and commit or roll back. |
| `status [PRODUCT] [--json]` | Join central desired/previous state with each host's observed rollout state **and with the host's own software report**. A host that has not published rollout status is `unreported`, never healthy by assumption — and a host whose software report is missing, stale, `unmanaged` or at a version the fleet does not declare makes the command **exit non-zero**, naming the host and the exact disagreement in one sentence per row. |
| `logs PRODUCT --target TARGET [--version V] [--stream out\|err\|both] [--lines N] [--json]` | Read the candidate's own `stdout`/`stderr` off the target host: `{logs_root}/{product}-{version}.{out,err}`, the exact files the release agent opens for a candidate it spawns. Read-only, over the registry ssh channel. Defaults to both streams and the last 40 lines of each, and reports a file that is missing separately from one that is present and empty. |
| `doctor PRODUCT [--target TARGET] [--json]` | One verdict — `settled`, `rolling` or `blocked` — over desired versus observed release, the rollout phase and its detail, the candidate's port, liveness and readiness answer, the host's quarantine map with the desired digest called out, and the host's claiming gates. Read-only: it starts nothing, stops nothing and writes nothing. |
| `quarantine list PRODUCT [--target TARGET] [--json]` | The digests this host refuses to roll out again, each with the agent's own reason, when it was quarantined, and whether it is the digest the registry currently wants. Read-only. |
| `quarantine clear PRODUCT --target TARGET --digest SHA256 --reason TEXT [--json]` | Retire exactly one quarantined digest so the agent retries it. Backs the state file up, rewrites it atomically, and appends an audit line. Starts, stops and restarts nothing. |
| `rollback PRODUCT [--json]` | Atomically swap the previous exact release back into desired state with a new rollout generation. |

`prepare` accepts only an externally produced qualification record. It does not
run or infer qualification. `promote` rejects `pending` and `failed` records,
untrusted keys, mixed source revisions, missing target platforms, or any byte
that differs from its signed manifest.

### `stado release status` fails on silence

Until 2026-08-18 this command printed
`brama target=control-host desired=0.2.27 observed=unreported` and exited
**zero**. Both halves of that line were declarations: `desired` is what the
registry wants, `observed` is what the release agent last wrote about itself, and
a host that had never written anything was rendered indistinguishable from a
healthy one — in the command an operator reaches for to ask exactly that. On the
same day two machines were running a skarbiec built on a laptop (0.2.1 on one,
0.2.3 on the other, neither in any published release), the older of the two was
stripping the `brama:agent:<id>` tags off a live credential every rotation, and
nothing on any screen could name the program doing it.

A third column closes both gaps. It is the host's own software report
(`stado host software`), read out of `~/.stado/observations.json` rather than
gathered here, because a status read must not cost one ssh connection per target
— and because the *age* of the report is itself the finding when nobody has
refreshed it.

Five states fail, and each prints one sentence naming the host and the exact
disagreement:

| State | The sentence, and why it is a failure |
|---|---|
| No report | `control-host has never reported what software it runs…`. This is the state that used to print as `observed=unreported` beside exit 0. |
| Stale report | `… last reported its software stale (3h), past the window an observation speaks for…`. A report older than `observations::DEFAULT_TTL` is history and must never be read as the present. |
| Refused read | `… could not report its software (unverified): <the channel's own words>`. The previous report is not left standing looking current. |
| Unmanaged program | `… runs skarbiec 0.2.4 at $HOME/.stado/bin/skarbiec: its digest 2e059a3abd19 matches no release artefact Stado published, so it is unmanaged`. |
| Version disagreement | `… and the fleet declares 0.1.3`, on the same line as the row it is about. |

What is deliberately **not** a failure is a program nothing declares. A host's
`$HOME/.stado/bin` accumulates dated backup copies — this laptop carries eleven
of `stado` alone — and none of them is running. Failing on those forever is how
an operator learns to write `|| true` after the command, at which point the drift
it exists to catch stops being noticed again; `stado service converge` refuses to
fail on an unmeasured binary for the same reason. Every such program is still
reported and still counted in `stado host software`. It just does not decide the
gate.

The scope of the gate is therefore exactly what the fleet declares it manages:
each name in the target's `managed_versions`, plus the release-control product's
own binary at `<install_root>/<binary>` — which lives under the product's install
root and so appears in no `managed_versions` entry and no `$HOME/.stado/bin`
listing. Accountability is resolved against the live registry on every read
rather than frozen into the record, so a declaration added an hour after a report
brings that program into scope immediately.

`StadoDesktop`'s Releases screen shows the same verdict in a `Software` column
and lists the findings verbatim in the pane below. It reads `verdict`, `failed`
and `findings` straight out of `status --json` and re-derives none of them: one
command decides what `unmanaged` means.

On macOS, install the reconciler as the registry target's runtime account:

```bash
deploy/install_release_agent.sh TARGET RUN_AS_USER HOME STADO_BIN STADO_CONFIG
```

The LaunchDaemon drops to `RUN_AS_USER` before reading or writing canonical
storage and uses non-interactive `sudo` only for system launchd cutover and the
declared candidate account.


### `stado release install-local`

The delivery contract's local endpoint. A delivery declaring a `target` in the
product manifest is pinned to run on that registry host; there this command
verifies the delivered archive against the digest the delivery worker provides
(`WISENT_RELEASE_ARCHIVE`, `WISENT_RELEASE_SHA256`), extracts the member named
by `--member` (default `bin/stado`), and installs it under `$HOME/.stado/bin`
by rename with a dated backup. Installation is a local file operation on the
host receiving the software, so no delivery needs ssh or Remote Login — the
release that installed over ssh died on the first host without it. It replaced
the last load-bearing script of the 137 retired on 2026-08-19.

### `stado release status` shows the pipeline itself

Below the per-target rollout rows, the command lists the newest pipeline runs:
identity, state, each platform leg with its job id, the queue's live word on a
job in flight, and — for a running build — crates compiled so far against the
previous run of the same platform, labelled the estimate it is. A failed run
or platform carries its persisted failure with the job's own last output
lines. The Releases screen in Stado Desktop renders the same `--json` payload,
so the CLI and the GUI cannot disagree.

### `stado release logs`

The command that ends the guessing. A brama candidate died in under ninety
seconds and the rollout state said only
`candidate did not become ready within 90s: pid 46748 is gone` — the outside
view of a process that had already written down why it exited, in
`/Users/charles/.stado/logs/brama-0.2.27.err` on the host, where nothing in the
CLI would read it.

```bash
stado release logs brama --target control-host --version 0.2.27 --stream err --lines 40
```

`--version` defaults to the desired version, which is the version any candidate
on the host is running; name it explicitly to read the logs of a release that
has since been rolled back. `--json` prints
`{"product","target","version","streams":[{"stream","path","bytes","lines","state"}]}`,
where `state` is `read`, `empty` or `missing`. Those last two are not the same
finding and are never collapsed: `empty` means the agent opened the file, so the
spawn happened and the product said nothing, while `missing` means the rollout
never got as far as opening it. `bytes` is the whole file's size, so a 40-line
tail never reads as the whole log.

### `stado release doctor`

Every fact in the paragraph above was individually visible before this command
and none of them were ever assembled, so answering "will this rollout land, and
if not, what is holding it" meant reading a registry document, an object in the
store, a JSON file on the host and a capacity row, in that order, by hand.

```bash
stado release doctor brama --target control-host
```

```
product           brama
target            control-host
desired           0.2.27
observed          0.2.26
phase             quarantined
detail            candidate did not become ready within 90s: pid 46748 is gone
candidate         port=- health=no_candidate pid_alive=-
gates             disk_pressure_unresolved=true free_gb=2.1 low_watermark_gb=15
verdict           blocked
blockers          desired_digest_quarantined, disk_pressure_unresolved

next: stado release logs brama --target control-host --version 0.2.27 --stream err
```

The verdict is `blocked` when the desired artefact's digest sits in the host's
quarantine map — the agent then skips that exact release on every pass until
`stado release quarantine clear` runs or a new version is promoted — or when the
host's disk gate is unresolved, which is the state in which the queue agent
claims nothing at all. It is `rolling` while a candidate is staged or running or
while observed still differs from desired, and `settled` only when the host's
observed release equals the desired one with nothing in flight.

A gate that cannot be read is an error, not a missing field: a verdict computed
as though the gate were fine is exactly how a host that had stopped claiming for
hours read as healthy. `--json` prints
`{"product","target","desired_version","observed_version","phase","detail","candidate":{"port","health_status","pid_alive"},"quarantined":[{"digest","reason","quarantined_at","is_desired_digest"}],"gates":{"disk_pressure_unresolved","free_gb","low_watermark_gb"},"verdict","blockers"}`.
`health_status` is `ok`, `http_<code>`, `unreachable`, `no_candidate` (the state
names no candidate to probe) or `unprobed` (the target declares no readiness
path).

### `stado release quarantine`

The way back. The agent quarantines a digest that failed to become ready and
then never tries it again, which is right — a candidate that dies in ninety
seconds must not be respawned in a loop — but until this command existed there
were exactly two ways to retry one: open an editor on
`{state_dir}/<product>.json` on the host, or publish a new version number so the
digest changes. The first is an unaudited write to the file a rollout is driven
from, the second burns a version to say "try again", and the operator refused
both.

Read what the host is refusing, then retire the one entry:

```bash
stado release quarantine list brama --target control-host
stado release quarantine clear brama --target control-host \
  --digest 119f93dd06634e9249eef8ae633d2bc02139c588f19fe05f1c7864224182c9ef \
  --reason 'stderr named a missing config key; fixed and republished in 0.2.28'
```

```
cleared 119f93dd06634e9249eef8ae633d2bc02139c588f19fe05f1c7864224182c9ef for brama on control-host
  it was quarantined at 2026-08-17T09:14:02.113+00:00 because: candidate did not become ready within 90s: pid 46748 is gone
  previous state backed up to /Users/charles/.stado/release-state/brama.json.quarantine-backup-20260818T142530Z
  audited in /Users/charles/.stado/release-state/brama.quarantine-audit.jsonl
  nothing was started, stopped or restarted; the release agent rolls this digest out on its next tick
```

`--reason` and `--digest` are both required and neither is ever defaulted: a
blank reason is refused, and the digest must be the 64-character hex string
`quarantine list` prints, so a clear cannot be aimed by guessing. `--target` is
required for `clear` because the command rewrites that host's rollout state;
`list` may omit it only while the product rolls out to a single host.

`clear` removes one map entry and nothing else. It does not restart the service,
cycle a unit, kill a process or touch the desired version, and it deliberately
leaves `phase` and `updated_at` exactly as the agent left them — those are the
agent's account of its own last tick, and no tick happened. On its next pass the
agent finds the desired digest no longer quarantined and rolls it out by the
ordinary path.

Every guard exists because the agent is writing this same file every fifteen
seconds. The live file's digest must still be the digest the command read, or the
write is refused rather than discarding another writer's work; the previous bytes
are copied to `<product>.json.quarantine-backup-<UTC>` beside the state before
anything is written; the audit trail is proven appendable before the state is
touched, because an unaudited mutation is worse than a refused one; and the new
document is hashed *after* it lands on the host's disk and compared against what
the command built, so a short or interrupted transfer is discarded instead of
renamed over a working rollout. Only then does one `mv` inside one directory
publish it. The staging file is a copy of the live one truncated in place, so the
mode and owner the agent gave its state file survive the rewrite.

The audit trail is `{state_dir}/<product>.quarantine-audit.jsonl`, mode `0600`,
one JSON object per line: `actor`, `host`, `product`, `digest`, `reason`,
`audited_at`, `state_backup`, plus the `quarantine_reason` and `quarantined_at`
the entry carried — clearing an entry deletes those from the state file, and an
audit trail that destroys the evidence for the change it records is decoration.
It lives beside the document it describes so the next reader of that state file
finds the account of why it looks the way it does without leaving the directory.

`list --json` prints
`{"product","target","entries":[{"digest","reason","quarantined_at","is_desired_digest"}]}`,
and an absent state file is an empty `entries` with the human rendering saying so
by path: the agent has never reconciled this product on that host, which is a
different problem from nothing being quarantined. `clear --json` prints
`{"product","target","digest","cleared","reason","audited_at","state_backup"}`.

## `stado storage`

| Subcommand | Behavior |
|---|---|
| `stado storage copy` | Copy queue state from one storage backend to another. Writes. |
| `stado storage ls [PREFIX]` | Per-prefix object counts across the canonical set, or the objects under one prefix. |
| `stado storage stat PATH` | One object: `present`, `absent` or `unreachable`, with size, timestamp, metadata and version token. |
| `stado storage cat PATH` | Write one object's body to stdout. |
| `stado storage verify` | Compare two stores object-for-object. Read-only. |
| `stado storage put URI SOURCE [--if-absent]` | Write a provider-neutral product object. `stado://releases/...` is always create-only. |
| `stado storage get URI DESTINATION` | Read a product object; releases use the dedicated public GET route when remote. |
| `stado storage objects NAMESPACE [PREFIX]` | List mapped product objects. |
| `stado storage rm URI` | Delete a product object; release deletion is always rejected. |
| `stado storage url URI` | Render the authenticated object URL or dedicated release URL. |

Queue inspection commands read the one store selected by `STADO_CONFIG`;
`copy` and `verify` take both stores as explicit operator-only flags. Product
commands use `stado://` names and the Stado API when `STADO_API_URL` is set.
The direct CLI cannot overwrite or delete a release object: PUT implies
create-if-absent for the `releases` namespace and RM fails before any backend
or remote request.

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
| `list --unowned [--json]` | Product processes running on any host that no launchd job or systemd unit owns. |
| `status NAME [--json]` | One service everywhere it is managed. A `failed` row earns a `failure:` block under the table: launchd's last exit status for the label and the unit's stderr tail, gathered best-effort from the host. |
| `converge TARGET [SERVICE] [--apply] [--json]` | Declared revision against the revision the host reports. |
| `restart NAME [--host TARGET] [--json]` | Restart one unit; no recovery pass. The output names the launchd domain the restart acted in. A system LaunchDaemon is restarted by ending its owned process for launchd's `KeepAlive` to replace, or refused with the cause and the privileged command named — see below. |
| `stop NAME [--host TARGET] [--json]` | Stop one unit, including a process it disowned. |
| `show NAME [--host TARGET] [--json]` | What the unit actually runs: program and arguments. |
| `adopt UNIT --host TARGET [--json]` | Bring an existing unit under management. |
| `retire UNIT --host TARGET [--json]` | Bootout/disable and forget; files kept. |
| `deploy NAME --host TARGET --from PATH [--json]` | Render, push, bootstrap, record. |
| `deploy NAME --host TARGET --from-artifact REF [--json]` | Install one published version, then the above. |
| `update NAME --host TARGET --from-artifact REF [--json]` | Move a unit already managed onto a new version. |
| `update NAME --host TARGET --from-archive PATH [--json]` | The same, from a local bundle no store carries yet. |
| `update NAME --host TARGET --rollback-to VERSION [--json]` | Point `current` back at a version already on the host. |
| `ensure NAME --host TARGET --from PATH [--arg A]... --reason WHY [--json]` | Assert the unit that host must be running. Idempotent, works where the per-user launchd domain does not exist, and never unloads a unit. |
| `directory show [--json]` | The service directory: active host, per-caller endpoint, consumers. |
| `directory profiles [--json]` | Placement profiles: services, start/stop order, hosts, required state. |
| `directory endpoint NAME [--target T] [--json]` | The address the directory declares for a target. |
| `directory connect NAME [--target T] [--no-verify] [--json]` | A route derived from placement, proven to answer, with no fallback. |
| `directory bind NAME [--target T] [--json]` | Serving parameters for the placed host: listen address and encrypted peers. |
| `directory consumer-add NAME CONSUMER [--capability C]...` | Declare that a consumer may use a service. |
| `directory consumer-rm NAME CONSUMER` | Remove a consumer's declaration. |
| `logs NAME [--host TARGET] [--lines N] [--json]` | Tail the unit's log, then its stderr as its own section — launchd keeps the two in separate files. A unit whose plist declares no `StandardErrorPath`, or whose stderr file is empty, says so instead of showing nothing. |
| `env NAME [--host TARGET] [--json]` | Effective environment, secrets redacted. |

`NAME` accepts either the logical service name or the host's own name for
the unit (launchd label, systemd unit name), so `service restart weles-api`
and `service restart com.wisent.weles-api` are the same request. Omitting
`--host` acts on every host that manages the name.

### Deploying a version rather than a path

`--from` takes the absolute path, on the target host, of a program that is
already there; the unit is rendered around it and nothing versions it.
`--from-artifact` takes a published reference instead: it resolves to an
immutable version, places that version under
`~/.stado/services/NAME/<version>/`, verifies the sha256 the manifest declares
**before** anything is linked, and moves `current` onto it atomically. A failed
install leaves the previous `current` running, and the unit points at `current`,
so a rollback is a relink rather than a redeploy.

Exactly one source is accepted. Neither is a safe default: a path deploys
whatever happens to be on the host, with no version anybody can name.

### The service directory answers "where is X"

`service_directory` keys each service's `endpoints` by the machine **asking**,
not by the machine serving, because these services bind loopback on their own
host and so the true address differs per client. `directory endpoint` resolves
against this target and refuses to invent an address when the target has no
entry — an undeclared endpoint means nobody has said how this machine reaches
the service, and a guessed loopback address is how a client ends up talking to
the wrong process.

`consumers` declares who may use a service. A system absent from that list is
not provisioned however well its code works.

These commands read and mutate the raw registry document. There is deliberately
no typed model of the block: a model drops the keys it does not know, and this
document has already lost keys that way.

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

### When the beacon says failed

The beacon says "it died" — more often than not with an empty detail. The
why lives on the host: launchd's last exit status for the label, and the
stderr the unit wrote before it went. `status` gathers both best-effort over
the read-only channels and prints them as a `failure:` block under the
table, one per failed unit:

```
failure: control-host com.wisent.weles-api: last launchd exit 1
  stderr: /Users/charles/.stado/logs/com.wisent.weles-api.log
  Error: listen EADDRINUSE: address already in use 127.0.0.1:8084
```

The stderr tail is the same file `service logs` reads, narrowed to the ten
lines a failure block can show, and the `stderr:` line names the file it
came from — or the reason there was nothing to show (`absent in plist`,
`<path> (empty)`). Every read can fail, because the host may be the thing
that is broken; a failed read degrades to a `note:` line in the block,
never to a failed `status`. A failed system LaunchDaemon's block carries
one more line saying what `service restart` can and cannot do to it —
the restart section below has the whole route.

`logs` tails that stderr file as its own section, headed
`== HOST UNIT stderr (PATH) ==`, whenever the plist declares a
`StandardErrorPath`; a unit with no stderr path declared, or with an empty
stderr file, is answered with that fact instead of with silence.

### Is the host running the build we shipped?

Every state above is about the *unit*, and every one of them stays true across
a release that never reached the box: `active` is still `active`, and the plist
still names the same program. `converge` asks the other question, per managed
binary the target declares a version for.

The declaration is `targets[].managed_versions` — the same one `host inventory`
reconciles against and the only one `host release` will deliver — never a git
commit. These hosts carry installed release artefacts and not checkouts:
`control-host` runs Weles out of `/Users/charles/weles`, which holds a
`package.json`, a `.weles-release` stamp, a `provenance.json` and no `.git` at
all, so a commit comparison there could only ever answer "unknown" about a
product that is in fact precisely versioned.

```json
"managed_versions": {
  "skarbiec": "0.1.3",
  "stado": "0.6.0",
  "weles-worker": "0.5.0"
}
```

Each value is an exact semantic version — a channel, an alias or a range cannot
be compared for equality, and equality is the whole of the question. Declare
one with `stado host declare-version TARGET --binary NAME --version X.Y.Z`;
`converge` refuses anything else before it contacts the host.

| Verdict | Meaning |
|---|---|
| `in-sync` | The host runs exactly the declared version. |
| `host-behind` | The host runs a version strictly OLDER than the declared one. This is the state that hid behind a passing `service list` for as long as it took somebody to notice the behaviour was old; `--apply` delivers the declared version. |
| `host-ahead` | The host runs a version strictly NEWER than the declared one: the declaration is the thing that is stale. Delivering the declared version would downgrade a live host, so `--apply` refuses to touch it and names the `stado host declare-version` command that moves the declaration to what the host runs. |
| `unknown` | Nothing usable came back: the reporter could not run, the channel refused, the binary is not installed, or the artefact carries no version metadata. |

The installed version comes from `report-installed-versions`
(`stado-rs/scripts/report-installed-versions.sh`), a read-only script embedded
in the stado binary itself and run as one fixed remote script — nothing is
installed on the host to produce the answer, and a host whose reporter cannot
run reports `unknown` with the remote's own words in the detail column. It
fetches nothing, restarts nothing, and prints one
`binary=<name> version=<installed|unknown> root=<path> unit=<label|none>
state=<launchd state>` line per declared binary. It reads a version from, in
order: the program itself for an owner-only Stado binary under
`$HOME/.stado/bin` (`--version`, then the `version` subcommand), then the
artefact's own `package.json` `/version`, then `.weles-release`, then
`provenance.json`.

**A product whose artefact carries none of those reports `unknown`, and an
`unknown` is never silently treated as `in-sync`.** It is printed as its own
verdict, named again on stderr with the path the reporter looked in, and after
`--apply` it fails the command. The remedy is to make the product stamp its
artefact, not to re-run `converge`.

Exit codes follow the same rule `service verify` follows, for the same reason:

| Invocation | Exit |
|---|---|
| every binary `in-sync` | 0 |
| any binary `host-behind` or `host-ahead`, without `--apply` | 1 |
| only `unknown`, without `--apply` | 0 |
| `--apply`, every binary confirmed `in-sync` afterwards | 0 |
| `--apply`, anything not `in-sync` afterwards | 1 |

Reporting fails on drift alone, so a reporter that could not run cannot
masquerade as drift — the same way a missing probe is never allowed to
masquerade as an outage. After `--apply`, `unknown` *is* a failure: an operator
who asked for convergence is owed proof of it, and "the reporter could not
answer" is not proof.

`--apply` delivers the declared version of every `host-behind` binary by
calling `stado host release --binary NAME --version X.Y.Z TARGET` in-process,
and then **re-reads** the installed versions through the same embedded
reporter. A delivery that reports `released` has testified about its own
work; the exit code of this command is decided by what the host says
afterwards. There is no second delivery path: a binary the product
declaration does not carry is reported as undeliverable, never attempted, and
`unknown` rows are never delivered to — delivery ends in a unit restart, and
restarting a working service because a report was missing is how a healthy
host goes down.

A `host-ahead` binary is refused outright: the host runs newer than the
declaration, so delivering the declared version is a downgrade of a live
host, and a converge that performs one is the registry's staleness shipped as
an outage. Each refusal names the exact
`stado host declare-version TARGET --binary NAME --version X.Y.Z` that moves
the declaration to the version the host is actually running, and the command
exits non-zero.

`converge` never writes the registry. The declared version is the operator's
statement of intent, published with `stado host declare-version`; a converge
that edited the document to match the host would turn a drift report into a
rubber stamp.

```bash
stado service converge control-host
stado service converge control-host stado --apply
```

### Which artefact is the live process actually running?

Every column above is about what is INSTALLED, and an installed version says
nothing about a process that started before it. Two incidents sat in exactly
that gap with every other column correct: Brama's process kept running an
artefact tree `current` no longer pointed at, and the Weles worker kept serving
a `dist` that was replaced 26 seconds after it started. `converge` therefore
carries two more facts per unit.

| Field | Meaning |
|---|---|
| `running_binary` | The executable the host's process table says the pid under that unit is running. `null` when nothing runs under the unit. |
| `binary_matches_process` | `true` when that executable is what the unit's declaration resolves to today AND neither file has been written since the process started; `false` when either is untrue; `null` when it could not be established. |

The `PROCESS` column prints `matches`, `differs` or `unknown` for those three
states, and a `differs` row also names the running path on stderr, because the
path is what an operator acts on and is far too long for a column. `unknown` is
never folded into either answer, for the same reason the verdict column keeps
its own `unknown`: a unit with nothing running under it has produced no
evidence about artefact identity, and `true` there is the report this field
exists to replace.

Both facts come from one extra read-only round trip per unit, on the same
channel: the unit's own file for what it declares, `readlink` for what its
`current` link resolves to now, the process table for the executable and the
start time, and `stat` for when each file was last written. The verdict is
computed in the CLI from those facts, never in the remote shell, so there is
one opinion about artefact identity. A unit the reporter names but the registry
does not declare is not asked about at all — locating its unit file would mean
guessing a path for a unit nobody adopted.

A row can read `in-sync` and `differs` at once, and that combination is the
whole point: the version on disk is the declared one and the running code is
not it. The remedy is `stado service restart NAME --host TARGET`.

```bash
stado service converge control-host --json | jq '.binaries[]
  | {binary, installed_version, running_binary, binary_matches_process}'
```

### Restarting a system LaunchDaemon

A unit in `/Library/LaunchDaemons` belongs to launchd's system domain, and
the approved channel is unprivileged: `launchctl bootstrap system` is not
available to it. `restart` still has a route, and reads both gates off the
host before anything is signalled. Every daemon this fleet installs carries
`UserName`, so its process runs as the approved user even though the job is
root's, and `KeepAlive` `<true/>`, so launchd puts a new process in place of
one that ends. Ending the process from the account that owns it is the same
sequence `launchctl kickstart -k` performs, minus the privilege: the job is
never unloaded, and there is no window in which it does not exist. The
detail column says so, because `restarted` beside a `kill` should not have
to be taken on trust.

When a gate does not hold — no `KeepAlive` or a conditional one, a process
owned by another account, or nothing running the unit's declared argv at all
— the command refuses rather than end a process nothing will respawn, names
the cause, and names the one privileged command that works:
`sudo launchctl kickstart -k system/<label>`. `stado host recover` is not
that command either: its pass reports each system daemon it skipped as
`needs_privileged_bootstrap` instead of re-bootstrapping one. The `DOMAIN`
column in `restart`'s own output names the domain the restart acted in,
because a restart in `user/501` and one in `gui/501` are different
operations.

### Processes no unit owns

Two `stado agent` processes ran on the always-on mac for four days with no
launchd unit behind them, executing a binary older than the one on disk. Every
other answer in this group is about declared units, so none of them could name
those processes, and `service deploy` could not replace them because it
bootstraps into the per-user launchd domain, which does not exist over ssh.

```bash
stado service list --unowned
```

```
HOST               PID    PRODUCT_GUESS  STARTED_AT                COMMAND
control-host  46748  stado          Fri Aug 14 09:12:33 2026  /Users/charles/.stado/bin/stado agent
```

A process counts as a product process when the executable it runs, or the entry
point an interpreter was handed, lives under a managed root: the install root of
every product in `stado-rs/data/products.json`, plus `~/.stado/services`, where
`service deploy --from-artifact` installs. Merely mentioning such a path is not
enough — a `tail` on a log under the root would match `pgrep -f` — and a report
that names those is a report operators learn to ignore.

Ownership is asked of the init system rather than assumed. On macOS the pids in
the `services` table of every printable launchd domain are owned, and so is any
descendant of one, which is why a job's own child processes never appear here.
On Linux the answer is the cgroup the kernel put the process in: a `.service`
cgroup belongs to a unit, a `.scope` to a login session that has since gone.

This is the one read in the group that cannot come off the beacons — an unowned
process is by construction in nobody's declaration — so it costs one read-only
ssh per `kind=local` host. It starts nothing, stops nothing and signals nothing,
and a host that will not answer is named on stderr with a non-zero exit rather
than dropped, because "no unowned processes" and "nobody looked" are not the
same answer. `--json` prints
`{"unowned":[{"host","pid","command","started_at","product_guess"}]}`.

`product_guess` names the product whose own file is being executed, not the root
it sits in: `stado` and `skarbiec` share `~/.stado/bin`, and reporting a
four-day-old unowned agent as possibly-skarbiec would be worse than saying
nothing. A program under `~/.stado/services/NAME/` is named by `NAME`, the name
`service deploy` created it with.

### Asserting a unit, where the per-user domain does not exist

`deploy` installs a unit and refuses one that is already declared, so nothing
could be run twice — or run from a script — to make a host run what it is
supposed to run. And on an ssh login there is no Aqua session:
`launchctl bootstrap gui/$uid` answers
`Could not switch to audit session ... Operation not permitted`, and `deploy`
returned that failure having installed nothing. That is how the two unowned
agents above came to exist.

```bash
stado service ensure weles-api --host control-host \
  --from /Users/charles/.stado/bin/stado --arg agent --arg --auto \
  --reason 'the queue agent has run with no unit behind it since 2026-08-14'
```

```
HOST               SERVICE    LABEL                                  DOMAIN  ACTION   PID
control-host  weles-api  com.wisent.stado.service.weles-api     system  created  908
audit record service_audit/control-host/20260818T142530.114203Z-com.wisent.stado.service.weles-api.json
```

| Action | When |
|---|---|
| `already_correct` | The unit declares exactly this argument vector and a live process under it is running that program. Nothing is touched at all. |
| `restarted` | The unit was already loaded and was kicked in place. |
| `created` | There was no unit, so one was rendered, written and bootstrapped. |

`domain` is `system` or `user`. Where the per-login launchd domain exists the
unit is a LaunchAgent in `~/Library/LaunchAgents`; where it does not, the same
job is rendered as a launchd **daemon** in `/Library/LaunchDaemons` with a
`UserName` naming the account it must run as — without that key launchd would
run the fleet's control binary as root against an account-owned `~/.stado`. The
daemon file is the one write this command cannot do as the login user, so a host
without passwordless sudo is told exactly that rather than left with a rendered
plist nobody loaded. `systemd --user` on Linux reports `user`.

An existing unit is only ever restarted **in place** (`kickstart -k`). It is
never unloaded and bootstrapped back: that sequence took the always-on host down
once, when launchd still held children of the old job, the bootstrap back failed
and the unit was left unloaded with its listeners gone. For the same reason a
loaded unit whose definition names a different argument vector is refused rather
than overwritten — launchd holds the definition it bootstrapped, so a rewritten
plist under a live job changes nothing an operator can see. Retire it first.

There is also no fallback to `launchctl submit` or to a bare background process,
which `deploy` still has: those two are how a host comes to run a program no
unit owns, which is the state `list --unowned` finds and this command exists to
end. A unit that will not stay up is reported as the failure it is, with
`stado service list --unowned` named, because the usual cause is a disowned
process still holding the port.

`--reason` is required and a blank one is refused. When a pass changes anything
— the host, the registry, or both — the reason is written as one create-only
object beside the canonical registry document, at
`service_audit/<host>/<UTC>-<label>.json`, carrying the action, the resolved
unit and its path, the domain, the pid, the program and argument vector, the
registry generation the write produced and who ran it. Beside the registry
rather than in the queue store because the registry is the state that changed,
and on a GCS deployment those are different buckets. A pass that reports
`already_correct` on an already-declared unit changed nothing and records
nothing: an audit trail that also records the passes which changed nothing is
one nobody reads.

The unit is recorded in the registry as managed, through the same validated
write path `adopt` and `deploy` use, so every other command in this group can
address it afterwards. A declaration that names a different file than the unit
was installed at — the agent path where the daemon path is now in force — is
corrected in one document write.

`--json` prints `{"host","name","label","domain","action","pid"}` and nothing
else; where the audit record landed goes to stderr, so the document stays
exactly that shape.

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
stado service adopt com.wisent.weles-api --host control-host
stado service restart com.wisent.weles-api
stado service logs com.wisent.weles-api --lines 40
```

## `stado resolver`

```bash
stado resolver resolve stado://service/brama \
  --consumer wisent-backend --json
stado resolver serve --target gpu-host
```

`resolve` discovers the registry authority from the local bootstrap registry,
fetches and validates its versioned snapshot over registry-owned SSH, enforces
the service's exact consumer capability policy, and returns only the logical
URI, routing generation, and capabilities. It does not disclose a host or
endpoint.

`serve` validates that `--target` is this machine, loads
`targets[].service_resolver` from the authority snapshot, binds its API and
adapters only on loopback, then watches that authority. `GET
/v1/resolve/service/<name>` requires `X-Stado-Consumer`; the response includes
the matching local adapter URL when one is configured. Each adapter resolves
again for every connection, connects directly when the service is local, and
otherwise uses the target's registry-owned SSH transport. Adapter streams close
after `idle_seconds` without traffic (330 seconds by default, above the model
gateway's own request deadline so a slow reply is never cut), bounding retained
client keep-alive sockets and their SSH transports. New connections fail closed
during placement transactions and after the cache freshness deadline.
If authority changes, resolvers adopt the new source only after the old
authority has delivered a valid snapshot naming it.

Install the host daemon with:

```bash
deploy/install_service_resolver.sh [registry-target]
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
| skarbiec-contract | WARNs when the configured broker rejects a read that names no field. Every whole-item read in the build fails against such a broker, and that is what silences a health beacon without saying why. The probe carries no consumer and no bearer: the handler validates `id` and `field` before it looks at identity, so an unauthenticated request reveals which contract is in force and nothing else. |

Checks are fault-isolated and individually deadline-bounded: one failure never
prevents the rest from running, and a black-holed endpoint becomes one FAIL row
instead of a hung command. The probe object is deleted even when the read-back
fails; a leaked one is named so it is obviously safe to remove.

## `stado instances list` and `stado resources`

`instances list` remains a read-only view of live agent VMs. Every operator
resource mutation uses the single `resources` command family and a versioned,
hash-pinned plan.

| Command | Behavior |
|---|---|
| `stado instances list [--provider P] [--json]` | Lists instance reference, provider, accelerator, age, ownership holders, and orphan state. |
| `stado resources show [--provider P] [--json]` | Produces the provider-neutral inventory used by planning. |
| `stado resources rationalize --output PLAN [--provider P] [--min-age 24h]` | Writes a canonical, expiring cleanup plan. It never mutates resources. |
| `stado resources kill-irrational --plan PLAN --expect-hash SHA [--approve ACTION] [--allow-irreversible] [--yes]` | Preflights the full selected action graph, then executes only automatic and explicitly approved actions. Without `--yes`, it is a read-only preview. |
| `stado resources shutdown --project PROJECT (--all-stado-owned \| --resource gcp:TYPE:LOCATION/NAME) --output PLAN` | Writes a GCP shutdown plan containing only reversible pause, resize-to-zero, stop, and Cloud SQL activation-policy actions. |
| `stado resources apply --plan PLAN --expect-hash SHA --yes` | Applies exactly one shutdown plan after checking its hash, expiry, configuration fingerprint, dependencies, and every precondition. |
| `stado resources verify --operation ID` | Compares live state with the applied or restored postconditions and archives `verification-latest.json`. |
| `stado resources restore --operation ID --yes` | Restores only actions this operation actually applied, in reverse dependency order. Already-desired resources are never “restored.” |
| `stado resources operations list` / `show ID` | Reads the plan, CAS state, and append-only events from the configured Stado storage backend. |

Explicit shutdown selectors are `gcp:scheduler:REGION/NAME`,
`gcp:zonal-mig:ZONE/NAME`, `gcp:regional-mig:REGION/NAME`,
`gcp:instance:ZONE/NAME`, and `gcp:cloud-sql:NAME`. `--all-stado-owned`
discovers VMs and MIGs covered by Stado's `wisent-agent-*`, `stado-*`, or
`wisent-*` naming contracts, plus the configured coordinator Scheduler job.
Cloud SQL has no repository-wide
ownership contract, so it must be named explicitly.

Plans contain typed action kinds and resource locators, never caller-supplied
HTTP methods or URLs. Execution performs every preflight before the first
mutation and repeats the relevant precondition check immediately before each
action. It aborts on the first unresolved error and records receipts and
observed state in both configured storage and `~/.stado/operations/`.
Re-running the same pinned plan reconciles an action interrupted after its
provider accepted the mutation. Irreversible cleanup requires both an explicit
action approval and `--allow-irreversible`; shutdown plans reject irreversible
actions at schema validation time.

## `stado secrets put|get|ls|rm`

Application credentials live in the separate Skarbiec service. `put` reads
from STDIN only—there is deliberately no `--value` flag because argv is
visible in process listings and shell history. Each service receives its own
finite consumer grant; do not mint wildcard runtime grants.

Product object verification uses `stado-object-api-verifier`, whose visible
items exactly match `object_api.namespaces`. Release creation uses the
distinct `stado-release-api-verifier`, whose visible items exactly match
`release_api.publishers`. Managed-service status/restart uses
`stado-service-api-verifier`, whose visible items exactly match
`service_api.deployers`; each consuming deployment owns only its mapped
deployer item token. Local and Azure workload agents use separate consumers
and may list only the application items declared in `agent.skarbiec.items`;
`secret_fields` is the smaller item/field projection available to jobs.

`get` is the only Stado subcommand that renders a value; `ls` reads metadata
alone. `put` creates an encrypted Skarbiec version and `rm` performs a
recoverable soft delete. Each action requires a matching action-qualified
grant scope and is recorded in Skarbiec's tamper-evident audit journal.

Cloud provider credentials are never workload-agent items. GCP, Azure, and AWS
resource access belongs to the enabled provider adapter's exact plugin identity
and SDK chain; agent grants containing `stado-gcp`, `stado-azure`, or
`stado-aws` fail profile validation. Application model, media, data, email,
push, object, scheduling, and release functions use their dedicated router or
client items rather than generic provider keys.


## `stado stream probe|declare|apply|status|pair|stop`

An interactive session on a fleet host, and the stream a client receives. This
is how a GPU in the fleet gets used interactively — for a desktop, a renderer,
or a game — because a board cannot be borrowed over a network: the process that
renders has to run on the machine that owns the card, and only the frames
travel.

| Command | What it does |
|---|---|
| `stream probe TARGET` | Read-only: boards with their PCI bus ids and UUIDs, driver version, DRM nodes, whether a display manager already owns the screen, free space on `/` and on the library volume, and the tailnet address a client would dial. Changes nothing. |
| `stream declare TARGET [--resolution WxH] [--refresh-hz N] [--gpu-uuid GPU-…] [--library-dir PATH] [--steam]` | Writes `targets[TARGET].display_stream` into the canonical registry. Refuses a library on the root volume, a non-`x11` session, a resolution outside 640..7680, a refresh outside 24..240, and an unpinned Sunshine. |
| `stream apply TARGET [--provision-library]` | Reconciles the host to that declaration: Xorg screen sized by the declaration on the declared board (`AllowEmptyInitialConfiguration`, so no monitor is needed), `openbox` to own the root window, Sunshine from a digest-pinned `.deb`, and two systemd units. Idempotent. |
| `stream status TARGET` | Units, the screen's real size, what is rendering, bound ports, paired client count, library space, and the address to point Moonlight at. |
| `stream pair TARGET --pin 1234 [--client NAME]` | Hands Moonlight's four-digit PIN to Sunshine's API over the managed host channel. This exists so pairing needs no browser: the web UI is never opened, and its credentials are generated on the host and never leave it. |
| `stream stop TARGET [--purge]` | Stops the session; `--purge` also removes the units and the Xorg screen, returning the host to headless. |

`declare` picks the Sunshine artifact from the host's own distribution, because
that is what decides whether it installs at all: the 26.04 package wants
`libc6 >= 2.43` and `libicu78`, and on Ubuntu 25.10 apt answers `[no choices]`
until the artifact matches. `--sunshine-url` with `--sunshine-sha256` pins one
explicitly for a distribution this build has no measured digest for.

`--provision-library` binds the declared library directory onto the host's
largest real filesystem when it would otherwise land on a root volume with no
room. Without it such a host is refused and its filesystems are named; with it,
the line written to `/etc/fstab` carries a `# stado-stream` tag so
`stream stop --purge` removes exactly its own.

The board is declared by driver UUID and Xorg is configured by PCI bus id;
`apply` resolves one to the other from the probe, because only the host knows
that mapping. On a two-card host, naming the second board keeps the session and
the job agent out of each other's way — the agent places work on the emptiest
card, so a session holding one board pushes batch work to the other.

What the fleet does **not** do here: install or launch a client. Moonlight runs
on the operator's own machine, and the pairing PIN comes from it.

Anti-cheat is the one thing no configuration fixes: titles with kernel-level
anti-cheat refuse to run on Linux, and streaming a Linux host does not change
that.

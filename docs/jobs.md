# Jobs

You have a command to run on fleet compute: how does it get from your shell to
a worker, what happens while it runs, and what do you do at each state? This
page follows one job through its whole lifecycle. First submission is covered
in [quick-start](quick-start.md); flag-by-flag detail lives in [cli](cli.md).

## Submit

```bash
stado submit "python train.py --epochs 3"
```

The job enters `queue/<id>.json` in the configured canonical backend. Routing
and sizing are declared at submit time: an optional provider constraint
(`--provider`, with `--pin-provider` or the default `--any-provider`),
spot policy, a hard `--max-cost-per-hour` cap, priority within the FIFO
bucket, a hard RFC 3339 `--deadline-at`, and the resource envelope
(`--gpu-type`, `--vram-gb`, `--machine-type`, `--exclusive`). Inputs are
declared, not ambient: a git repo pinned to an exact commit (`--repo`
plus `--repo-ref`), pinned artifacts (`--input-artifact`), and scoped
workload secrets as `--secret-env ENV_NAME=SKARBIEC_ITEM#FIELD`.

A batch is one submit over a file of commands — one command per line, with
blank lines and `#` comments skipped. With `--batch` the positional command is
ignored; every submitted job carries the same batch id, and `stado status`
filters by job id or batch id substring, so a batch is inspected as one unit.

```bash
stado submit --batch commands.txt ""
stado status <batch-id-substring>
```

## Submit profiles

A profile is a named, reusable submit spec. `--profile NAME` applies one from
the bundled profiles directory (or `$WC_PROFILES_DIR`); explicit CLI flags
override profile fields.

```bash
stado profiles              # list available profiles
stado profiles <name>       # show one profile's JSON
stado submit --profile <name> "python train.py"
```

## Queued → running: the claim

Job state is a set of storage prefixes, not a mutable status field. The
queued-to-running transition is a create-if-absent write of
`running/<id>.json`, so it has exactly one winner — two agents scanning the
same queue cannot both start your job. Every other prefix transition writes
the new record before deleting the old one; readers tolerate the retry window
and resolve terminal state first.

Who wins the claim is a scheduling decision. The scheduler orders the queue by
priority and creation time and admits only targets whose capabilities, policy,
deadline, resource envelope, and provider fence match. Every agent broadcasts
its free capacity to `capacity/<consumer-id>.json`; the cloud scheduler reads
these to decide whether to *yield* a queued job to a free local consumer
instead of dispatching a paid VM, marking jobs with the highest
$-saved-per-GB-of-local-VRAM for local pickup first. So a job you expected on
a cloud GPU may legitimately run on an idle local machine — see
[providers](providers.md) for the provider model. `--pinned-host` opts out:
only the named consumer may claim the job.

## Running: heartbeats and proof of life

A running job's evidence lives under `status/<id>/`. The job's per-job
heartbeat at `status/<job_id>/heartbeat` is written by the running job itself,
independent of the agent's capacity-broadcast loop. That independence is the
point: a long training subprocess can starve the broadcast loop until it looks
stale, and the reaper's heartbeat guard uses the per-job heartbeat as the
second signal — if any job on a VM heartbeats fresh, the reap is deferred.

Even the heartbeat can starve: a multi-GB checkpoint upload saturates outbound
network and delays the small heartbeat write while the job is demonstrably
alive. The guard therefore also accepts a fresh checkpoint write as proof of
life — the newest blob under the job's checkpoint prefix stays fresh
throughout an upload. A genuinely dead job writes neither, and is requeued
once both signals age out. A coordinator-side read or listing failure is never
treated as proof of death; the guard fails safe and defers.

What this means for you: a busy job is protected without any action on your
part, and a job whose worker really died is requeued automatically rather
than stranded.

Watch a job while it runs:

```bash
stado job watch <job-id> --follow
```

`--follow` polls the log until the job reaches a terminal prefix.

## Terminal states and results

`completed/` records success; `failed/` records failure with bounded error
classification. If the job declared `--verify`, that command must exit 0
after success — a non-zero exit reverses COMPLETED to FAILED, catching
silent-success failure modes. Output always lands under canonical
`status/<id>/output/`; `--output-uri` adds a second `stado://` destination.

```bash
stado results <job-id> ./out
```

The download includes the result manifest recording artifact size and SHA-256.
A failed job remains inspectable and may still publish logs and partial
artifacts.

## Cancel

```bash
stado cancel <job-id>
stado cancel <job-id> --terminate
```

Cancel works on a queued or running job and writes a durable record to
`cancelled/`. Without `--terminate`, a cancelled job's cloud VM keeps running
— and billing; `--terminate` also deletes the instance the job holds.

## Rerun

```bash
stado job rerun <job-id>
```

Rerun resubmits a job's exact spec under a new job id, from any lifecycle
prefix — the original record is untouched.

## Lifecycle table

| State (prefix) | Who sets it | What an operator does |
|---|---|---|
| `queue/` | `stado submit` (or the coordinator, for a due schedule) | Wait, or check why it is not claimed: eligible worker running, queue not paused, capacity fits, deadline not expired. |
| `running/` | The winning agent, via the create-if-absent claim | `stado job watch <id> --follow`; nothing else — heartbeats and checkpoint writes protect the slot. |
| `running/` (stale heartbeat and checkpoint) | The reaper requeues to `queue/` | Nothing; the job restarts from its last checkpoint. |
| `completed/` | The agent, after the job (and any `--verify`) succeeds | `stado results <id> <dir>`. |
| `failed/` | The agent, on job failure or a failed `--verify` | Read the log and error classification; fix; `stado job rerun <id>`. |
| `cancelled/` | The operator, via `stado cancel` | Confirm `--terminate` was used if the job held a VM. |

## Recurring jobs

A schedule submits a fresh job on a cron expression, with the same routing,
sizing, and secret-reference contract as a direct submit. Schedules live in
configured Stado storage and are evaluated every coordinator tick.

```bash
stado schedule create --cron "0 2 * * *" "python nightly.py"
stado schedule list
stado schedule pause <schedule-id>
stado schedule resume <schedule-id>
stado schedule run <schedule-id>
```

The default `--overlap-policy skip` does not fire while the prior instance is
still queued or running. `rm` deletes a schedule without affecting jobs it
already submitted; `run` fires once immediately regardless of the next run
time; `resume` recomputes the next run from now.

## What it cost

Cost reporting is computed from observed wall-times, per job and per batch:

```bash
stado cost report                  # $ spent per target_kind and per model, from completed jobs
stado cost estimate commands.txt   # project total $ for a batch file using observed per-job cost
```

`stado cost` also carries `allocation`, `forecast`, `anomalies`, and
`savings`; budgets and the wider cost model are covered in [costs](costs.md).

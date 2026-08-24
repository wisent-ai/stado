# Job

What is the durable object behind `stado submit`, and how does the fleet agree
on who runs it? A job is one queue object whose prefix is its state, claimed by
exactly one worker through a create-if-absent write.

## What it is

A job is a JSON object in the canonical backend. Its lifecycle is spelled by
the prefix it lives under:

| Prefix | State |
|---|---|
| `queue/<id>.json` | Pending; the submission ledger. |
| `running/<id>.json` | In flight, with owner, instance reference, and start time. |
| `completed/<id>.json` | Successful terminal state. |
| `failed/<id>.json` | Unsuccessful terminal state, with bounded error classification. |
| `cancelled/<id>.json` | Durable operator cancellation record. |
| `status/<id>/...` | Heartbeats, redacted output, result manifest, artifact evidence. |

The queued-to-running claim is create-if-absent and therefore has one winner.
Other transitions write the new record before deleting the old one; readers
tolerate the resulting retry window and resolve terminal state first, and
writers are idempotent and fenced by the expected generation.

## Who declares it

A submitter, through `stado submit` (or a batch file) into the configured
canonical queue backend, via the authenticated Stado machine/object boundary.
The job spec — provider constraint, spot policy, cost cap, resource envelope —
travels with the object; `stado job rerun` resubmits a job's exact spec under a
new id.

## Who observes it

The Rust scheduler reads a bounded provider-neutral queue window, orders by
priority and creation time, and admits only targets whose declared
capabilities, policy, deadline, resource envelope, and provider fence match.
Agents claim through the atomic storage primitive before writing runtime state
or starting a process.

Two liveness signals keep a running job's owner alive in the reaper's eyes:

- The agent's capacity broadcast at `capacity/<consumer_id>.json` — the
  primary signal, which can starve while a long training subprocess runs.
- The per-job heartbeat at `status/<job_id>/heartbeat`, written by the running
  job itself via the agent's status watchdog and deliberately not coupled to
  the broadcast loop. If any job assigned to a VM has a fresh heartbeat, the
  agent is alive and the reap is deferred — reaping a productive VM destroys
  hours of work and forces a restart from the last checkpoint, or step 0 if
  none exists.

Operators observe with `stado status` and `stado job watch`.

## Where it lives

Under the queue prefixes above, in whatever backend the selected
`STADO_CONFIG` names — the same canonical prefixes regardless of backend. See
[object-store](object-store.md).

## Commands

```bash
stado submit "python train.py"
stado status <id>
stado job watch <id>
stado job rerun <id>
stado results <id>
stado cancel <id>
stado queue pause
```

The end-to-end submission workflow is [jobs](../jobs.md); flags are in
[cli](../cli.md).

## Not to be confused with

- **A [lease](lease.md)** — time-bounded ownership with a TTL. The
  queued-to-running claim is create-if-absent and does not expire on a clock;
  liveness is proven by heartbeats instead.
- **A [service](service.md)** — a unit a host must keep running indefinitely.
  A job terminates.
- **A schedule** — `stado schedule` submits jobs on a cron; each submission is
  an ordinary job object.

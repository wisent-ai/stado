# Runbook

Something in the fleet looks wrong — which command do you run first, and what
does its answer mean? Each entry below starts from the symptom, names the
first command, says how to read it, and names the next one. Flag-by-flag
detail lives in [cli](cli.md).

## A host went quiet

First command:

```bash
stado host ping <target>
```

`ping` probes two independent signals — ssh reachability and health-beacon
age — and its verdict is the worse of the two (`ok`, `stale`, `down`). The
exit status follows the combined verdict, so a box that answers ssh with a
five-day-old beacon fails this command; that split exists because a host once
answered ssh perfectly for five days while its beacon writer was wedged.
Read it as two different repairs: ssh up with a stale beacon is a host that
is running and not reporting; both signals down is a host that is
unreachable.

Next command:

```bash
stado host link <target>
```

`link` is the host's own account of why it went quiet: beacon age, the
tailnet path and endpoint it last published, its last sleep and wake, recent
interface changes, the silences recorded against it, and what readers refused
because of them. A silence record opens when the newest beacon crosses the
fleet silence threshold (default 300 seconds, `STADO_SILENCE_THRESHOLD_SECONDS`)
and its `started_at` is the last moment the host was heard from, so the
duration is the outage, not the polling interval. Reader refusals are counted
over the last hour by reason — `directory_cache_stale`,
`authority_unreachable`, `beacon_stale` — and each record carries the
refusing component's own sentence verbatim, so the string you grep for is one
that exists in a source file. The exit status follows the verdict; blockers
are named in the report.

## A service reads missing, failed, or unknown

First command:

```bash
stado service list
```

`list` answers from the latest health beacons alone — no ssh — so it still
reports on hosts that are currently broken. `STATE` is the host's own word
about its unit; `OBSERVED` is when anybody last confirmed the service from
outside. A beacon older than the fleet silence threshold turns every unit on
that host to `unknown`, with the age and the threshold spelled out in
`DETAIL`: `health beacon is <age>s old, past the <threshold>s silence
threshold; unit state is unknown`. Read `unknown` as "the host said nothing
usable", which is deliberately not the same answer as `missing`.

Next commands:

```bash
stado service status <name>
stado service logs <name> --host <target>
```

`status` adds best-effort host reads — launchd's last exit status and the
stderr tail — for units whose beacon state is `failed`; those reads degrade
to a note, never to a failed command. `logs` tails the unit's log over the
approved channel.

The autonomy loop will also act on this without you. Every `stado optimize
run` and scheduled tick joins the beacon's unit state with a fresh endpoint
sweep: a `failed` or missing unit is reasserted through the idempotent
`service ensure` path, a live process running a stale copy of its own
declared binary is kicked in place, and a process executing a binary the unit
never declared stays refused as `identity_unresolved`. The verdict lands in
`state/autonomy/services/latest.json` and an immutable
`state/autonomy/services/runs/<timestamp>.json`; `stado optimize status`
prints the latest report. Whether the plan is only recorded or actually
executed depends on the autonomy mode — see [autonomy](autonomy.md) and the
[missing service reconciliation table](operations.md#missing-service-reconciliation).

## A write to the object API answers 401

The sentence is `401 {"error":"unauthorized or non-immutable release
write"}`, and it names neither the prefix nor the grant. The object gateway
authorizes a write by matching its key against the configured namespace's
prefix allowlist; a key whose prefix is outside that allowlist can never
authorize, whatever token you present.

First command:

```bash
stado config show
```

Read `object_api_namespaces`: each namespace carries its verifier `item` and
its `prefix_policies`, each with a `prefix` and the `actions` it allows. If
the key you are writing does not start with a listed prefix, that is the
whole diagnosis. This is exactly why every autonomy object and every
host-silence record is rooted under `state/`: no namespace declared
`autonomy/` or `host_silence/`, so the whole layer's writes were refused
with this sentence, and `state/` is a canonical prefix that is authorized
wherever the queue prefixes are. The fix is to put the object under an
allowlisted prefix — `state/` for fleet state — not to widen the token. A
verifier item that cannot be read answers 503, not 401, so a 401 is always a
scope or bearer answer.

## Reads look healthy while writes fail

Storage is single-writer with automatic read failover: mutations commit to
the configured primary and mirror to a read-only disaster-recovery backend, a
failed primary read may be served from the backup, and the backup is never
promoted to writer. The trap is the asymmetry: every write can be failing
while reads keep answering — from stale backup data. This ran in the field:
autonomy writes were refused for days while the `local` backup backend kept
serving stale reads, which is why `stado optimize status` still printed a
confident forecast.

So when writes fail, distrust every fresh-looking read and check its own
timestamp through the fleet plane: `reported_at` on beacons (`stado service
list` prints the age when it is past threshold), inventory freshness in
`stado optimize status`. A confident answer with an old `reported_at` is a
stale read, not health. There is one object plane — the configured Stado
backend; the writer has no cloud CLI, provider SDK, direct bucket URL, or
cross-backend fallback. See [disaster-recovery](disaster-recovery.md).

## A job is stuck

First commands:

```bash
stado status <job-id>
stado machine logs <job-id> --cursor 0 --limit 1048576
```

`status` gives the queue's view; `machine logs` pages the canonical command
log by byte cursor.

Then check proof of life before concluding anything is dead. The running job
writes a per-job heartbeat at `status/<job_id>/heartbeat` (via the agent's
status watchdog), deliberately decoupled from the agent's capacity broadcast:
a fresh heartbeat means the agent is alive and busy, and the reaper defers.
The second proof is a fresh blob under the job's checkpoint prefix — a
multi-gigabyte checkpoint upload saturates outbound network and starves the
small heartbeat PUT, so a stale heartbeat with fresh checkpoint writes is a
job that is alive and mid-upload, not an orphan. Only when both signals have
aged out is the job genuinely dead and requeued.

To classify a failed job fast, grep its stdout against the
[failure mode quick-grep table](operations.md#failure-mode-quick-grep). For
the job lifecycle itself, see [jobs](jobs.md).

## `ensure` refuses

`stado service ensure` is idempotent and honest: each of its refusals in the
field names its own repair.

- **A per-login unit on an always-on host.** The declaration puts the unit in
  launchd's user domain, but nobody is logged in graphically on an always-on
  host, launchd builds no `gui/<uid>`, and the system domain is the only one
  that host can load a unit into. The finding is printed as one sentence
  ending in the one privileged install command
  (`sudo /usr/bin/install -m 644 -o root -g wheel <plist> /Library/LaunchDaemons/...`)
  to run on the host. `stado service list` and `stado registry doctor` report
  the same finding before any restart trips over it.
- **A loaded unit running a different program.** The refusal reads
  `<domain>/<unit> is loaded and runs [<declared argv>], not [<argv>]; retire
  it first`. launchd holds the definition it bootstrapped, so rewriting the
  plist under a live job changes nothing an operator can see; `ensure`
  refuses rather than silently overwrites. Retire the unit
  (`stado service retire <unit> --host <target>`), then ensure again.
- **Registry push refused on directory generation.** A registry write
  replaces the whole document, so a push that would delete top-level keys, or
  whose `service_directory.generation` would go backwards (which would make
  every stale cached directory start looking current), is refused. The
  refusal spells the repair: re-pull, re-apply the edit, and push again;
  `--force` only if the deletion or the older directory is genuinely the
  intent.

## A release did not land on a host

First command:

```bash
stado service converge <target>
```

Three verdicts: `in-sync`, `drifted`, and `unknown` for a binary whose
installed version could not be read — `unknown` is never folded into either
of the other two, so an uninstalled reporter cannot masquerade as drift.
Reporting exits non-zero on drift alone; `--apply` delivers the declared
version through `stado host release` and exits non-zero unless every binary
in scope is confirmed `in-sync`.

If the host keeps refusing a version it was already given, check quarantine.
The release agent quarantines a digest that failed to become ready and never
retries it on its own — correct, since a candidate that dies in ninety
seconds must not respawn in a loop.

```bash
stado release quarantine list <product> --target <target>
stado release quarantine clear <product> --target <target> \
  --digest <digest> --reason "<why this digest gets another chance>"
```

`clear` starts nothing and kills nothing: it removes one map entry, and the
agent's next tick finds the desired digest no longer quarantined and rolls it
out on its own. `--reason` is required and recorded in the audit trail beside
the host's rollout state. See [primitives/release](primitives/release.md).

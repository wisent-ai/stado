# Walkthrough: service deploy plus self-repair

What does one declared service look like when it is healthy, what does it
look like when it breaks, and what does the autonomy loop do about it without
you? This page walks that story as commands and their readings. No terminal
output is invented; every quoted sentence exists in the source that prints it.
The evidence rules live in [operations](operations.md) under "Missing service
reconciliation"; the loop's modes and rails live in [autonomy](autonomy.md);
symptom-first triage lives in the [runbook](runbook.md).

## Declare and ensure

Stado ships a catalog of preconfigured Wisent services, deployable by name:

```bash
stado service catalog
stado service ensure <name> --host <target> --reason "<why this host runs it>"
```

`service deploy <name>` and `service ensure <name>` resolve the catalog when
nothing else declares the unit; a custom service is declared first with
`stado service declare --file <declaration.json>` (required keys: `name`,
`host`, `source.artifact`, `source.sha256`). `ensure` is the idempotent path:
it reads what is there first, reports a unit already running the declared
program as `already_correct` with nothing touched, kicks a stopped unit in
place (`kickstart -k`, never unload-and-bootstrap), and installs a unit on a
host that has none. A loaded unit whose definition names a different program
is refused rather than overwritten. `--reason` is required because `ensure`
installs and restarts, and every such change is recorded beside the registry
document that declared the unit.

## Healthy

```bash
stado service list
```

`list` answers from the latest health beacons alone — no ssh — so it keeps
answering while a host is wedged. `STATE` is the host's own word about its
unit; `OBSERVED` is when anybody last confirmed the service from outside, a
different question answered by a different party (`never` means no machine
has ever confirmed it from any vantage). A healthy row is a beacon-reported
running state with a fresh `reported_at`.

## Broken

Three readings, each deliberately distinct:

- **`failed`** — the unit exists and the host says nothing runs under it.
  `stado service status <name>` adds best-effort host reads (last exit
  status, stderr tail) for `failed` units; those reads degrade to a note,
  never to a failed command. `stado service logs <name> --host <target>`
  tails the unit's log over the approved channel.
- **`missing`** — the beacon is fresh and omits the declared unit. The
  `DETAIL` column reads `declared here; the latest beacon does not report
  it`.
- **`unknown`** — the beacon itself is stale or absent. A beacon older than
  the fleet silence threshold turns every unit on that host to `unknown`,
  with `DETAIL` spelling out exactly why: `health beacon is <age>s old, past
  the <threshold>s silence threshold; unit state is unknown` (or `health
  beacon has no usable reported_at; unit state is unknown`). Stale evidence
  is never allowed to produce a confident `active` or `missing`.

## What one optimize run does about it

```bash
stado optimize run
```

Every run and every scheduled autonomy tick joins two independent facts:
the unit state in the newest beacon and a fresh `stado service verify`
reachability sweep from the declared consumer hosts. Neither fact may stand
in for the other. A `failed` unit is the same repair as a missing one — the
unit exists, nothing runs under it — and both go through the idempotent
`service ensure` path, which never unloads. `unknown` evidence mutates
nothing, with one exception: a silent host's declared beacon unit is
reasserted over the host channel, because the beacon's own death is what made
everything else `unknown` and the channel answering is the evidence that
repair is possible.

Whether the plan executes depends on the [autonomy](autonomy.md) mode. In
`report` (the default), the repair is recorded as `planned` with the detail
`report mode: <action> was planned but not executed`; under an emergency
pause the detail is `mutation blocked by autonomy emergency pause`. In
`enforce-safe`, the reversible repair executes, bounded by
`max_actions_per_tick`, the emergency pause, the circuit breaker, and a
per-service mutation lease on `service:<host>:<unit>`.

## Where the verdict lands

The result is written to `state/autonomy/services/latest.json` and an
immutable `state/autonomy/services/runs/<timestamp>.json` per run.

```bash
stado optimize status
```

prints the latest report alongside the mode, pause and circuit-breaker state;
`--json` returns the same document machine-readably. Each outcome row carries
the host, service, unit, beacon state, endpoint state, a classification, the
action taken, and a detail sentence:

| Classification | Meaning |
|---|---|
| `planned` | Report mode or emergency pause: the mutation was recorded, not executed. |
| `reconciled` | The repair executed and its running postcondition was verified. |
| `identity_unresolved` | A live process or responding endpoint could not prove it is the declared program; Stado refuses to duplicate or kick it. |
| `declaration_incomplete` | Nothing declares the unit's `program` and `args`, so the repair cannot render the unit from the document. |
| `endpoint_unverified` | Endpoint absence was not proven — the sweep did not complete, or the probe answered `unverified` — so no change was made. |
| `externally_managed` | The unit belongs to the fixed host-recovery program, which is never silently converted into a registry service. |
| `lease_blocked` | `another reconciler owns this service mutation`; recorded instead of racing it. |

`identity_unresolved`, `declaration_incomplete`, and `endpoint_unverified`
alert once on the transition. Only a mutation that actually failed on a host
feeds the circuit breaker; these refusals are computed before any host
command runs, and a refusal must not starve the healthy repairs behind it.

## The honest refusals, and your part

Each refusal names its repair:

- **`identity_unresolved`** — prove ownership or retire the imposter: read
  the unit with `stado service show <name>`, and retire a unit running the
  wrong program (`stado service retire <unit> --host <target>`) before
  ensuring again.
- **`declaration_incomplete`** — the durable fix is the document: read the
  truth with `stado service show <name>`, write `program` and `args` into the
  registry entry, and every future repair renders from the declaration.
- **`endpoint_unverified`** — fix the probe, not the service: the sweep
  could not prove absence, so re-run `stado service verify` from the
  consumer hosts and repair whatever kept the probe from running.
- **`externally_managed`** — leave it with host recovery; declaring it as a
  registry service is a deliberate operator act, never an autonomous one.
- **`lease_blocked`** — wait: another reconciler holds the lease and the
  next tick re-evaluates from fresh evidence.

Once the repair lands, the next beacon reports the unit running,
`stado service list` reads healthy again, and the next run's report records
zero missing. Flag-by-flag command detail lives in [cli](cli.md).

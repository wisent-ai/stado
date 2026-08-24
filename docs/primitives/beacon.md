# Beacon

How does the fleet know a host is alive without asking it? Each host
publishes a small health document on a timer; every liveness verdict in Stado
is a read of that document's contents and age, never an ssh probe.

## What it is

A beacon is one JSON document per host: the host's own slug, a `reported_at`
timestamp, a disk line, and a `units{}` map with one entry per declared unit,
each carrying its `state` (`deploy/host_health_beacon_macos.sh`). The unit
list is not fixed in the script — "the labels this host is judged on are the
ones the registry declares for it", matched by target name or declared
hostname with the `.local` suffix dropped. For each label the writer asks
launchd in both the GUI and system domains, and a live process outranks
history: the previous run's exit code is read only when nothing is running
now.

The fleet's beacons are published on a one-minute timer
(`stado-rs/src/monitor/host_silence.rs`).

## Who declares it

The host itself. Linux and macOS writers collect local disk and service
state, then call `stado host publish-beacon FILE` — an authenticated PUT
through the scoped host-health API, requiring `STADO_HOST_HEALTH_API_URL`
plus the dedicated `stado-host-health-beacon` Skarbiec grant. Missing
routing, an unreadable or over-broad grant, an insecure non-loopback HTTP
URL, failed authorization, and backend errors all leave the prior beacon
untouched and return failure; there is no cloud CLI, provider SDK, direct
bucket URL, ambient credential, or cross-backend fallback in the writer
([operations](../operations.md)).

### The relay

A machine with no stado binary can still collect its own beacon, but it
cannot hand it in — and one published by hand goes stale within the hour,
which is worse than none because it still looks like reporting. So a host
that has the binary and the grant relays: on every tick it already runs, it
collects over the approved channel and publishes on the silent host's behalf
(`deploy/host_health_beacon_macos.sh`). The relay speaks only for hosts
nobody is reporting: it asks the store which beacons are stale and skips any
host fresh within the relay window, because an earlier relay carrying an
older unit list overwrote a correct document with a thinner one, and a
just-installed service read as missing for as long as the relay kept winning.

## Who observes it

`stado host health <target>`, `stado registry beacon-age`, `stado registry
doctor`, and the read side of the service layer: `stado service list` joins
the declared managed set against the latest beacons and issues no ssh at all,
because the moment you most need to ask "what is supposed to be running here"
is the moment the host has stopped answering
(`stado-rs/src/deploy/service.rs`). The autonomy cycle's service
reconciliation reads the same documents ([operations](../operations.md)).

Readers resolve a [target](target.md) to its beacon object through the slug
rule in `monitor/host_health.rs` — target name, declared hostnames, and their
first dot-labels, in order.

## Where it lives in the store

`host_health/<slug>.json`, inside the deployment's namespace — the latest
document only, one per host (`monitor/host_health.rs`). Because the prefix
holds only the latest document, a gap closes over itself the moment the host
comes back; the durable record of the gap is the silence record,
`state/host_silence/<host>/<started_at>.json`, opened when the newest beacon
crosses the threshold and closed by the first fresher beacon
(`monitor/host_silence.rs`).

## Silence, and what stale means

A host counts as silent when its newest beacon is older than the fleet-wide
threshold: `STADO_SILENCE_THRESHOLD_SECONDS`, default 300 seconds — five
minutes, because at a one-minute publication interval three consecutive
misses is a host that has stopped talking and one miss is a slow `pmset`
call. A value that is not a positive integer falls back to the default rather
than disabling the detector (`monitor/host_silence.rs`). No beacon at all is
silent — a host that has never published is not a host that is fine. A beacon
stamped in the future is not silent: clock skew on the publisher is not an
outage.

A stale beacon cannot describe the present, so nothing derives a confident
verdict from it. A unit read through a stale beacon is `unknown`, never
`active` and never `missing` (`deploy/service.rs`;
[operations](../operations.md): "a stale beacon is `unknown`, never
`missing`"), and staleness never authorizes a host mutation, with the single
beacon-unit exception [operations](../operations.md) documents. Unit states a
fresh beacon can report: `active`, `inactive`, `failed`; `missing` means the
beacon exists and does not carry the unit; `unknown` means no beacon or an
empty state (`deploy/service.rs`).

## Commands that act on it

```bash
stado host publish-beacon <file-or-dash>
stado host health <target>
stado registry beacon-age
stado host ping <target>
stado host link <target>
```

`host link` answers "why did this host go quiet": beacon age, recorded
silences, and the reader refusals filed against it. Full flags in the
[cli](../cli.md) reference.

## Not to be confused with

- **A probe.** A beacon travels outward from the host; nothing dials in.
  Endpoint reachability is a separate observation (`stado service verify`).
- **A silence record.** The beacon is the latest state; the silence record
  is the durable history of its gaps ([observation](observation.md)).
- **The registry.** The [registry](registry.md) declares which units the
  host is judged on; the beacon reports what launchd or systemd actually
  says about them.

# Observation

How does Stado know when anyone last actually looked at a fact — and how does
it keep "nobody looked" from reading as either "it is fine" or "it is down"?
An observation is the record of one look, with its author and its age.

## What it is

A declaration does not decay: `active` written twelve days ago reads exactly
like `active` confirmed a second ago. That is how a service directory entry
pointing at a closed laptop stayed structurally valid for twelve days while
every consumer routed into a forward with no upstream and a worker refused
29,616 times to claim work. The model could not express the sentence "nobody
has looked since" (`stado-rs/src/observations.rs`).

An observation is a separate kind of record, carrying four things a
declaration does not:

| Field | Meaning |
|---|---|
| `fact` | What was checked, spelled identically by every checker (`<kind>:<subject>`), so two commands looking at one thing produce one row. |
| `vantage` | Who looked, by registry target name. Reachability has no fleet-wide answer — a loopback endpoint is true from its own host and false from everywhere else — so an observation without a vantage is not evidence of anything. |
| `state` | `observed`, `unreachable`, or `unverified`. |
| `at` | When, RFC 3339 UTC. The field the outage needed and did not have. |

Three states, never two. `unreachable` means someone looked and it was not
there — a failure. `unverified` means the look could not happen: host down,
helper not installed, channel refused. Collapsing those into one `false` is
how a fleet learns to ignore its own reports. And `never` is the fourth honest
answer: this fleet has no record of anyone ever checking this. Reading it as
`observed` is the original bug; reading it as `unreachable` invents an outage.

Freshness is a property of the record, not the reader. A `Fresh` observation
was made inside the caller's TTL and may be acted on. A `Stale` one still
carries the last thing anyone saw, clearly marked as history, but must never
be treated as the present. The default TTL is one hour: a laptop lid closes in
a second, and an hour is how long the fleet is willing to be wrong about it —
visibly amber within the working hour rather than silently in twelve days.

## Who declares it

Nobody. An observation is precisely not a declaration; it is recorded by
whatever process actually looked, stamped with its own vantage. Writers
include:

- `stado service verify` — one record per probed finding, `unverified` ones
  included (`stado-rs/src/cli/service_verify.rs`).
- `stado host software <target>` — persists the host's software report under
  `software:<name>@<host>` with the target as the vantage, so it goes stale
  exactly as a reachability observation does ([cli](../cli.md)).
- `stado service directory connect` — records whether the proven route
  answered. A record that cannot be written must not turn a successful
  connect into a failure; the answer stands whether or not the host can keep
  notes (`stado-rs/src/cli/directory.rs`).

## Who observes it

- `stado service directory show` prints each endpoint with its freshness on
  one line — read alone, `from operator-host: http://127.0.0.1:8080` is a
  claim with no author and no date, the exact rendering an operator believed
  for twelve days (`stado-rs/src/cli/directory.rs`).
- `stado service list` carries an observed column per row.
- `stado release status` reads software reports out of the observation file
  rather than opening one ssh connection per target, and a report older than
  the TTL, or absent entirely, makes it exit non-zero ([cli](../cli.md)).

## Where it lives

`~/.stado/observations.json`, owner-only, written through a temporary file in
the same directory and a rename — a reader must never see half a file, and a
reader here is a routing decision (`stado-rs/src/observations.rs`).

## Commands

```bash
stado service verify
stado service directory show
stado host software control-host
stado release status
```

Flag-by-flag detail lives in [cli](../cli.md).

## Not to be confused with

- **A declaration.** The [registry](registry.md) and
  [directory](directory.md) say what should be true; an observation says who
  last checked and what they saw. The reconciliation between the two is the
  subject of [operations](../operations.md).
- **A host beacon.** A [beacon](beacon.md) is the host's own push about its
  units and disks; an observation is a look taken from a named vantage, which
  may be another host entirely.
- **`unreachable` vs `unverified`.** "I looked and it is gone" and "I did not
  look" send an operator to two different machines, and are never collapsed.

# Service

What is a managed service, as opposed to the launchd or systemd unit it names?
The declaration lives in the registry; the unit lives on the host; and the
whole service layer exists because the two used to be unrelated.

## What it is

A managed service is one entry in a [target](target.md)'s `services[]` array:
the launchd/systemd unit, program, arguments and host-side unit path Stado is
required to keep running ([architecture](../architecture.md)). The absence of
such a layer is the incident it closes: on the July control-host outage
`com.wisent.weles-api` existed on the box and was wedged, but nothing in
Stado declared it — so no command could list it, restart it, or even assert
that it was supposed to be running (`stado-rs/src/deploy/service.rs`).

A declaration carries the name the CLI addresses it by, the unit identity
(launchd `label` or systemd `unit`), the unit-file `path`, and — when the
declaration is the source of the unit rather than a pointer at a plist
somebody installed by hand — `program` and `args`. That pair is what makes
the declaration reinstallable from the document alone: `service ensure`
renders the unit from them, so a host that lost its unit file can be made to
run the right thing again (`deploy/service.rs`, `ManagedService`). A
declaration that names only a path cannot be reinstalled from the document;
repair records `declaration_incomplete` and alerts until the entry carries
its `program` and `args` — read the truth with `stado service show <name>`
and write it into the entry ([operations](../operations.md)).

## Sources: registry, recovery, product

The managed set has three sources, and the distinction is load-bearing
(`deploy/service.rs`):

| Source | Meaning |
|---|---|
| `registry` | Declared in the target's `services[]`. What adopt/retire/deploy edit, and what `stado registry doctor` diffs against live host state. |
| `recovery` | The fixed list every `stado host recover` pass restarts. Genuinely managed, but by that fixed program and not by the document — so it can be neither adopted nor retired, and is never silently converted into a registry service. |
| `product` | Located by a shipped product declaration naming both the label and the unit file, addressable without a registry record. |

## Who observes it

Two deliberately separate halves (`deploy/service.rs`). The read side —
`stado service list` — joins declarations against the latest host
[beacons](beacon.md) and issues no ssh, so it stays answerable while a host
is wedged; a stale beacon yields `unknown`, never a confident `active` or
`missing`. The write side (restart, deploy, ensure, retire, logs, env) rides
the approved host channel with a fixed, narrow remote program. The autonomy
cycle joins beacon unit state with a fresh `stado service verify`
reachability sweep and repairs from that evidence
([operations](../operations.md)).

## Where it lives in the store

In the [registry](registry.md) document, as `targets[].services[]`. Every
mutation goes through the same validated compare-and-swap write path as
`stado registry push`, so a mutation that would produce an invalid document
is refused with nothing uploaded ([cli](../cli.md)).

## Ensure

`stado service ensure` asserts the unit a host must be running, idempotently:
it installs the unit only where the host is not already running it, restarts
in place, and never unloads anything (`deploy/service.rs`). One pass reports
exactly what it did — `created`, `restarted`, or `already_correct` — and a
declaration that names its own label is rendered at that label, so an
existing unit is reinstallable from the document without becoming a second
service competing for the same port. Ensure is also the autonomy cycle's
repair path for a proven-missing unit, and the one repair permitted on a
silent host's own beacon unit, because the channel answering is the evidence
that repair is possible ([operations](../operations.md)).

## Adopt

`stado service adopt` brings a unit that already exists on the host under
management. Adoption requires proof: Stado records a corrected path or unit
record only when the unit is loaded and its live process matches the declared
program; unproven ownership is recorded as `identity_unresolved` rather than
guessed, and never duplicated ([operations](../operations.md)). The registry
record is built from what the host actually reported — the resolved unit id,
path, and init system — not from what the operator hoped
(`deploy/service.rs`, `record_from_report`).

## Retire

`stado service retire` removes a service from management: bootout/disable and
forget ([cli](../cli.md)). It is deliberately not "remove this service" —
`stado service remove` is the operation that also stops the unit and deletes
its declared file. Removing the last declaration drops the `services` key
entirely, so a host with nothing declared reads the same as one that never
declared anything (`deploy/service.rs`). Recovery-sourced units are never
written into the registry by any of these paths
([operations](../operations.md)).

## Commands that act on it

```bash
stado service list
stado service show <name>
stado service ensure <name> --host <host>
stado service adopt <unit> --host <host>
stado service retire <unit> --host <host>
stado service restart <name>
stado service logs <name>
```

Full flags and the deploy/declare contract in the [cli](../cli.md) reference.

## Not to be confused with

- **The unit itself.** The unit is host state; the service is the fleet's
  declaration of it. `service restart weles-api` and
  `service restart com.wisent.weles-api` address the same declaration by its
  logical name or its unit id (`deploy/service.rs`).
- **A service directory entry.** `stado://service/<name>` routing — who may
  call a service and where it currently answers — is the
  [directory](directory.md), a different primitive in the same document.
- **A release.** Installing signed product bytes is the
  [release](release.md) flow; the service declaration is what must keep
  running regardless of which bytes are current.

# Service directory

How does a workload find a service without knowing which host runs it? It asks
the service directory, the routing block of the registry that maps logical
service names to the one host currently serving each of them.

## What it is

Workloads address `stado://service/<name>` — never a host, tailnet address, or
service port. The directory resolves that name to the active placement host.
`service_directory.authority` names the one target and Stado binary allowed to
serve canonical snapshots and commit routing changes; every other resolver
fetches that versioned snapshot over the authority target's registry-owned SSH
transport and never treats its bootstrap registry copy as current routing.

`service_directory.generation` is the routing epoch. `placement move`
delegates to the authority, updates the active host for every service in the
placement group, and increments the epoch in the same compare-and-swapped
registry commit that moves the service declarations. Resolution fails closed:
while the transaction lock exists, resolution for that profile is refused;
resolver caches reject generation rollback and stop accepting connections
after `max_stale_seconds` without a successful authority refresh.

Each service entry carries two address maps that mean different things:

| Field | Meaning |
|---|---|
| `endpoints[<host>]` | The address that host **calls** to reach the service. This is what `service directory publish` writes into each host's forward marker, and the only map probing uses. |
| `standby[<host>]` | The address that host **would serve on** after a move. Nothing is supposed to answer there yet. |

The two used to share one field, and a verifier that read it the other way
reported a standby host as unreachable. The ambiguity was settled in the model,
not in any one command: `endpoints` is the address a host calls and nothing
else.

## Who declares it

Operators, through registry commits. `placement move` is the only path that
changes which host is active, and it goes through the authority.
`service directory consumer-add` declares that a consumer may use a service;
resolution applies that consumer capability policy.

## Who observes it

Every registered host runs a loopback-only Stado resolver that consumes the
authority snapshot. `stado service verify` goes further: it probes each
declared endpoint **from the host that is told to call it**, because probing
from the serving host proves the process is alive and proves nothing about
whether the fleet can reach it. Each entry gets one of three states, never two:

| State | Meaning |
|---|---|
| `observed` | Something answered at the declared endpoint, from the consumer's own host. The declaration is true right now. |
| `unreachable` | Nothing answered. The declaration is false. |
| `unverified` | The probe could not run — host down, channel refused, remote stado too old. "I did not look" and "I looked and it is gone" send an operator to two different places. |

The distinction exists because of a real failure: on 2026-08-11 the directory
declared `stado-object-api` active on a laptop. The laptop was closed, the
forward had no upstream, and a worker refused 29,616 times over twelve days to
claim work whose diagnostics it could not upload — while `config validate`,
`registry validate`, and `doctor` passed throughout, because none of them was
ever about reachability. Standby addresses appear in the sweep as their own
`unverified` rows: visible, never failures.

## Where it lives

The `service_directory` block of `registry.json` in the configured canonical
backend, served as versioned snapshots by the authority. Each host materializes
its own view as forward markers under `$HOME/.stado/forwards/`, written by
`service directory publish`; `stado host inventory` reconciles marker against
socket table and marker against directory as deliberately independent axes.

## Commands

```bash
stado service directory show
stado service directory endpoint <service>
stado service directory connect <service>
stado service directory publish
stado service verify
stado placement move
```

Flag-by-flag detail is in [cli](../cli.md).

## Not to be confused with

- **The [registry](registry.md)** — the directory lives inside it, but the
  registry declares what should exist; the directory answers where it is
  reachable from here.
- **A [service](service.md) declaration** — the unit a host must keep running.
  The directory routes to it; it does not manage it.
- **A [beacon](beacon.md)** — the host's own report of its unit state, one of
  the facts reconciliation joins with the directory's endpoint sweep.

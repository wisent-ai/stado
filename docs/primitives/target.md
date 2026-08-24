# Target

What is a registry target, and why is it not the same thing as a hostname?
A target is one routable box the fleet has written down; a hostname is one of
the several names that box may answer to.

## What it is

A target is one entry in the `targets[]` array of the canonical
[registry](registry.md) — "one routable box", in the words of
`stado-rs/src/targets.rs`. The registry is the single source of truth for
every box the queue can route to: workstations, GCP zonal dispatchers,
vast.ai pools. A target carries, among other fields:

| Field | Meaning |
|---|---|
| `name` | The fleet's word for the box. Lowercase `[a-z0-9._-]`, validated by the registry-v2 contract. |
| `kind` | `local`, `gcp`, or `vast`. |
| `hostnames[]` | Every name the machine itself answers to. |
| `ssh` | The channel destination (`[user@]host[:port]`), stored verbatim. |
| `role`, `host_heuristic` | Stable placement class and the declarative selector that resolves to exactly one local target. |
| `services[]` | The units Stado must keep running there — see [service](service.md). |
| `managed_versions` | The version each stado-managed binary is required to be at; optional, and a target that omits it is reported `undeclared`, not agreeing. |

Unknown per-target keys round-trip through the target's `extra` map, so a
writer built from an older checkout cannot delete a field a newer one added
(`targets.rs`, `ComputeTarget`).

## Who declares it

An operator, through `stado registry host add HOST --ssh DEST
--release-platform PLATFORM` or by editing the document and running
`stado registry push`. Declaration reads nothing from the machine — it is an
assertion, and `stado registry doctor` is what later diffs the assertion
against reality ([cli](../cli.md)).

## Who observes it

Everything that routes or repairs. The scheduler admits jobs only against
targets whose declared capabilities match ([architecture](../architecture.md)).
`stado host inventory <target>` reconciles the declaration against the live
host. Beacon readers resolve a target to its `host_health/<slug>.json` object.
The coordinator's rogue-daemon kill switch exits when a successfully read
registry omits this machine's entry (`targets.rs`).

## Where it lives in the store

`registry.json`, in the backend `STADO_CONFIG` selects — one document for the
whole fleet ([architecture](../architecture.md), canonical object layout).

## A target is not a hostname

The fleet knows the box by `name`; the machine knows itself by whatever
`hostname` returns, and the two are routinely different spellings of one
machine. The beacon writer (`deploy/host_health_beacon_macos.sh`) publishes
under the machine's own short hostname, lowercased — so the target and its
beacon file "are also spelled differently on a machine named twice", as that
script's own comment puts it, and every reader must try the name and every
hostname the registry declares for it.

The matching rule is `beacon_slugs` in
`stado-rs/src/monitor/host_health.rs`: for each identity — the declared
`hostnames[]`, the target `name`, the identity the caller asked for, and the
hostname inside the `ssh` destination — take both the first dot-label and the
full normalized form (trimmed, lowercased, trailing dot stripped), skip empty
or `/`-containing candidates, and deduplicate preserving order. The beacon
script applies the same idea from the other side: it matches the local
hostname against the target name and declared hostnames "with the local
suffix dropped", so `some-mac.local` and `some-mac` are one identity.

The practical rule: when a machine's self-reported hostname differs from its
target name, put every spelling in `hostnames[]`. A spelling the registry
does not declare is a beacon nobody can find.

## Commands that act on it

```bash
stado registry host add HOST --ssh DEST --release-platform PLATFORM
stado registry self
stado registry pull
stado registry doctor
stado host inventory <target>
stado host health <target>
```

Full flags in the [cli](../cli.md) reference.

## Not to be confused with

- **A hostname.** One target may carry several; matching uses the stem rule
  above, never string equality on the raw name.
- **An SSH destination.** `ssh` is one field of the target, stored verbatim;
  the target name is what every command takes.
- **A service.** A target is a box; a [service](service.md) is a unit
  declared on a box.
- **A beacon.** The target is the declaration; the [beacon](beacon.md) is
  what the box itself reports.

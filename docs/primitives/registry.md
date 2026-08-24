# Registry

Where does the fleet write down what should be true, and how does a write to
it stay safe? The registry is the one canonical document; everything else is
observation of it.

## What it is

`registry.json` is the fleet's declaration of what SHOULD be true
([architecture](../architecture.md)): the versioned compute-target and
coordinator registry, plus the `service_directory` and `placement_profiles`
blocks. It is "the single source of truth for every box the queue can route
to" (`stado-rs/src/targets.rs`). One document carries the
[targets](target.md), the [services](service.md) each target must run, the
service [directory](directory.md), placement profiles, build recipes, and the
fleet queue namespace.

Top-level keys a reader does not model round-trip verbatim through
`Registry::extra`, so a write never deletes them. This is load-bearing, not
cosmetic: a registry write replaces the whole document, and on 2026-08-04 the
canonical document lost `channels`, `enrollment` and `fleets` exactly that
way — divergent builds writing the same object, each erasing what it could
not name (`targets.rs`; `cli/registry.rs`).

## Who declares it

Operators and the validated write path. `stado registry push` uploads a local
file; `stado service adopt|retire|deploy` and `stado registry host add` edit
the document programmatically through the same path. Validation runs before
any store call, so a document that would not validate never reaches the
registry (`cli/registry.rs`, `push_document`).

## Who observes it

Every reader in the fleet. `fetch_registry_remote` is the fleet-survival
authority — the coordinator's rogue-daemon kill switch and host-health target
resolution read it, with a 30-second in-process cache. It returns a fetch
error rather than an empty registry, because "the store is unreachable" and
"the registry does not list you" drive opposite decisions: collapsing both
into an empty registry is what took the fleet down when the GCP billing
account was closed and the kill switch fired fleet-wide against a registry
nobody had touched (`targets.rs`).

A reader is not required to die with the authority. Every canonical read that
parses and passes the registry-v2 contract is copied to
`~/.stado/cache/registry-last-good.json` with a dated sidecar, and readers
that must keep answering serve that copy — carrying its age and one sentence
for the operator. The snapshot bundled with the binary sits below the cache
and is reachable only through the auto loader, announced whenever it is used
(`targets.rs`).

## Where it lives in the store

`registry.json` at the root of the backend `STADO_CONFIG` selects
([architecture](../architecture.md)). The read and write sides address the
same object on every backend; the group used to hardcode a GCS bucket, which
on an Azure-only deployment meant the one document the survival check reads
could be repaired by nobody (`cli/registry.rs`).

## Pushes: compare-and-swap and generations

A push is fenced, refused-by-default, and verified (`cli/registry.rs`,
`upload_payload`):

1. Read the current object and its store generation.
2. Refuse a payload that would delete a top-level key the current generation
   carries, unless `--force` — the 2026-08-04 accident, as a guard.
3. Refuse a payload whose `service_directory.generation` is lower than the
   one already published, unless `--force`. That counter is what consumers
   compare their cached directory against, and it only means something if it
   never goes backwards: on 2026-08-12 the directory went from generation 10
   back to 5 and two corrected endpoints reverted with it. Resolver caches
   reject generation rollback ([architecture](../architecture.md)).
4. Compare-and-swap against the read generation (or atomically create when
   the object is absent), then read back and verify both the generation and
   the exact bytes.

```bash
stado registry pull
stado registry validate registry.json
stado registry push registry.json
```

## Declaration is not observation

The registry declares; it never reports. `stado host inventory` and the host
beacons are the observation side, and the reconciliation between the two is
deliberately kept as independent axes — declared version vs installed binary,
declared unit vs beacon state, declared endpoint vs consumer probes
([architecture](../architecture.md)). Software that installs builds onto
hosts without being able to state the declared version, read the actual
version, and name the difference is not automation. A field like a target's
`managed_versions` is the declaration half; a target that omits it is
reported `undeclared`, never as agreeing.

## Commands that act on it

```bash
stado registry validate
stado registry push
stado registry pull
stado registry self
stado registry doctor
stado registry host add HOST --ssh DEST --release-platform PLATFORM
stado registry beacon-age
```

Full flags in the [cli](../cli.md) reference.

## Not to be confused with

- **The service directory.** The directory is a block inside the registry
  with its own generation counter and authority — see
  [directory](directory.md).
- **A beacon.** The [beacon](beacon.md) is what a host observes about
  itself; the registry is what the fleet declares about the host.
- **The object store.** The registry is one object in the store, not the
  store; job state lives under its own prefixes
  ([architecture](../architecture.md)).

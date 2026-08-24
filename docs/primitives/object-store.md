# Object store

Where does fleet state actually live, and who is allowed to write it? In one
canonical object plane, selected by `STADO_CONFIG`, where every write is
authorized by matching its key against a namespace's per-prefix action
policies.

## What it is

The object store is the single durable plane behind the fleet: queue state,
capacity broadcasts, control flags, the registry document, autonomy records,
and release artifacts all live under fixed `stado://` prefixes. The selected
`STADO_CONFIG` names the primary backend, replication, and release origin;
coordinator and agents use the same canonical prefixes regardless of backend.
There is no provider-derived storage fallback or direct client bucket path.

Writes cross an authenticated object boundary. The gateway resolves the
request's namespace from configuration, checks that the namespace's policy
allows this action on this key or list prefix, and then compares the bearer
against the namespace's Skarbiec-held token in constant time. A key no policy
covers — or an action the prefix does not allow — is refused with
`401 {"error":"unauthorized or non-immutable release write"}`, a sentence that
names neither the namespace, the prefix, nor the grant.

That refusal shape has bitten twice, the same way both times. The whole
autonomy layer originally wrote under `autonomy/`, which no namespace policy
declared, so placement, resource reconciliation, cost reports, and service
reconciliation had all been refused with that 401 since 2026-08-19 — while
reads kept working, so `optimize status` still printed a forecast. The fix was
to root every autonomy object under `state/` (`state/autonomy/...`), the same
move `state/host_silence/` had already made after the same 401 for the same
reason. If you add a new prefix, root it under a namespace some policy
declares.

## Who declares it

Operators, in the deployment configuration: the backend, the namespaces, and
each namespace's prefix/action allowlist. Writers are the coordinator, agents,
the autonomy layer, and authenticated submitters — each within its namespace's
policy. Reads under `stado://releases/` are the one public, bearer-free
surface; publication there is authenticated and create-only.

## Who observes it

Everything. The scheduler reads the queue window, the reaper reads capacity
and heartbeats, resolvers read registry snapshots, `stado optimize status`
reads autonomy records. One caution is load-bearing: the `local` backup
backend is a **read fallback, not a second truth**. During the autonomy 401
outage it kept serving stale reads, which is exactly why the layer looked
healthy while it had been unable to persist for days. A forecast you can read
proves nothing about your ability to write; `stado doctor` checks that the
backend the config names can be constructed, and a completed
`stado optimize run` proves the write path.

## Where it lives

In the backend the selected `STADO_CONFIG` names — Azure Blob and local
storage for the Azure and local outage profiles respectively. The canonical
layout (`queue/`, `running/`, `status/`, `leases/`, `capacity/`, `control/`,
`registry.json`, `state/...`, and the rest) is tabulated in the architecture
doc and is identical across backends. `system/storage-layout.json` is the
versioned layout marker.

## Commands

```bash
stado config show
stado config validate
stado doctor
stado storage
stado artifact list
```

`stado storage` moves queue state between backends (the billing-outage
migration path). Flags are in [cli](../cli.md).

## Not to be confused with

- **The [registry](registry.md)** — one versioned document inside the store,
  not the store itself.
- **The [release](release.md) channel** — immutable objects under
  `stado://releases/`, publicly readable through
  `https://stado.wisent.com/api/release/object`; copying release objects into
  a second "public" store is a broken release channel.
- **Skarbiec** — the credential store. The object gateway reads its verifier
  tokens from Skarbiec; the object store itself holds no provider or product
  secret material.

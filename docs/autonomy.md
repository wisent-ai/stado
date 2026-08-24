# Autonomy

What does `stado optimize` do on its own, and how do you keep it on a leash?
This page describes one autonomy cycle, the three modes that decide what a
cycle may execute, and the rails that stop it. Flag-by-flag listings live in
[cli](cli.md).

## One cycle

The coordinator runs the same cycle on every scheduled autonomy tick and on
every manual `stado optimize run`:

```bash
stado optimize run
```

Before anything else the cycle loads the shared control state — an emergency
pause or an open circuit breaker forces the rest of the cycle into paused
behavior — and refreshes the price book at `state/autonomy/cost/prices.json`
when it is older than the policy's pricing freshness TTL. Then, in order:

1. **Inventory.** Direct provider-API inventory, normalized into the
   provider-neutral resource model, enriched with prices, and published when
   its content changed. In report mode a cached snapshot fresher than the
   inventory TTL is reused instead of re-collecting.
2. **Placement.** Global placement of queued work across local capacity and
   the enabled cloud providers. This stage is skipped entirely in report mode
   and blocked when the inventory is incomplete. New cloud spend is bounded by
   the budget headroom computed from the [costs](costs.md) forecast.
3. **Resource reconciliation.** Findings against the policy's resource rules,
   executed through the existing immutable resource-plan engine. Every
   executed action passes the policy's fail-closed authorization and holds a
   mutation-slot lease, so at most `max_concurrent_mutations` run at once.
4. **Service reconciliation.** Registry-declared services are joined against
   fresh host-beacon and endpoint facts. The evidence rules and the
   per-evidence reconciliation table are documented once, in
   [operations](operations.md) under "Missing service reconciliation".
5. **Advice.** Rightsizing, scheduling, storage-lifecycle, network, and
   commitment recommendations are published. Advice is never executed by this
   stage; commitments in particular stay recommendations (see
   [costs](costs.md)).
6. **Cost reports.** Allocation, forecast, and anomaly reports are persisted,
   past decisions are measured against realized outcomes, and the savings
   ledger is summarized. Read them with `stado cost` — see [costs](costs.md).
7. **Lifecycle.** Retention-aware cleanup of the autonomy layer's own
   control-plane objects, capped like every other mutation
   (`deleted`, `bytes`, `capped` are logged).

The whole layer is deliberately not a parallel control plane: it reuses the
canonical queue storage, provider adapters, resource planner, and operation
journal, adding coordinator stages over the same state.

## Modes

The policy's `mode` decides what a cycle may execute. Read-only actions are
always allowed; financial commitments are denied in every mode.

| Mode | What executes |
|---|---|
| `report` (default) | Everything is observed and recorded, nothing is mutated. Placement is skipped, resource plans are not executed, and service repairs are recorded as `planned` with the mutation they would have made. |
| `enforce-safe` | Reversible actions on owned or adopted resources execute. New cloud placement gets zero spend headroom, so placement cannot add cloud cost. Destructive actions are denied. |
| `enforce-owned` | Reversible actions execute, destructive actions execute only when an explicit resource rule allows them, and this is the only mode in which new cloud placement receives a nonzero budget. |

With no persisted policy document, `load_policy` returns the default:
report-only, version `default-report-only`.

## Safety rails

- **Emergency pause.** `stado optimize pause <REASON>` stops new autonomous
  mutations; `stado optimize resume` clears it. The pause lives in shared
  control state, so every reconciler in the fleet honors it, and the reason
  and actor are recorded.
- **Circuit breaker.** Fed only by mutations that actually executed against a
  host: a failed execution increments the consecutive-failure count, a
  successful one resets it to zero. Reaching `circuit_breaker_failures` opens
  the breaker for `circuit_breaker_cooldown_seconds`, which the next cycle
  treats exactly like an emergency pause. Planning and read failures never
  feed it.
- **Action caps.** `max_actions_per_tick` and `max_actions_per_provider`
  bound each cycle; `max_deleted_bytes_per_tick` optionally bounds deletion
  volume.
- **Per-service mutation leases.** Before repairing a service, the reconciler
  takes a lease on `service:<host>:<unit>` with the policy's decision TTL. A
  second reconciler reaching the same service is recorded `lease_blocked`
  ("another reconciler owns this service mutation") instead of racing the
  first. Resource mutations take numbered mutation-slot leases the same way.
- **Fail-closed authorization.** Every non-read-only action is denied unless
  the policy allows it: incomplete inventory (when
  `require_complete_inventory` is set), resources that are not owned or
  adopted, protected production and stateful resources without an explicit
  rule, and actions estimated above `max_single_action_usd` are all refused
  with a recorded reason.

## Reports and status

Every autonomy object is rooted under `state/autonomy/` — the object gateway
authorizes writes by prefix, and [operations](operations.md) records the
outage that placed it there. Service reconciliation writes
`state/autonomy/services/latest.json` plus an immutable
`state/autonomy/services/runs/<timestamp>.json` per run; cost reports live
under `state/autonomy/cost/`.

```bash
stado optimize status
```

prints the mode, emergency-pause state and reason, circuit-breaker state with
its consecutive-failure count and open-until time, the policy version, the
latest inventory snapshot (resource count, completeness, snapshot id), the
decision count with active leases, and the latest forecast, anomaly, savings,
and service-reconciliation reports. `--json` returns the same document
machine-readably. Each placement or resource decision is immutable and
explainable:

```bash
stado optimize explain <DECISION_ID>
```

## Policy

The policy is one versioned JSON document. Inspect it, then replace it
atomically:

```bash
stado optimize policy show
stado optimize policy apply --file policy.json --expect-version <VERSION>
```

`show` prints the current version alongside the document. `apply` validates
the document fail-closed and writes it compare-and-swapped against
`--expect-version`; once a policy exists, `--expect-version` is required, so
two operators cannot silently overwrite each other. There is no separate undo:
rolling back is applying the previous document again, expecting the version
you are replacing. Budgets inside the policy are covered in
[costs](costs.md).

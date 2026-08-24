# Costs

What does the fleet cost, and what stops [autonomy](autonomy.md) from
spending more than you allow? Budgets live in the autonomy policy, the
forecast decides when new cloud spend is blocked, and two command groups read
the money: `stado cost` for attributed workload cost, `stado billing` for
what the providers themselves report. Flag-by-flag listings live in
[cli](cli.md).

## Budgets in the autonomy policy

The policy's `budgets` block holds five optional limits. Each must be a
finite non-negative number; an absent limit does not constrain.

| Field | What it gates |
|---|---|
| `hourly_usd` | Hourly spend ceiling. Headroom for new cloud placement is this minus the current hourly burn. |
| `daily_usd` | Daily spend ceiling. Divided by 24 it also caps the hourly headroom — the tighter of the two wins. |
| `monthly_usd` | Monthly ceiling. Headroom is this minus the projected end-of-month total. |
| `max_single_action_usd` | Any single autonomous action estimated above this is denied by policy authorization. |
| `max_commitment_usd` | Gates the commitment *recommendation* only. Purchases classified as financial commitments are denied in every mode: they require an operator-approved immutable plan. |

Apply budget changes like any policy change — `stado optimize policy apply
--expect-version` ([autonomy](autonomy.md)).

## The forecast and what `budget_exceeded` blocks

Every cycle builds a forecast from the attributed cost allocation, the policy
budgets, and the last billing snapshot: current hourly burn, end-of-month
projection, hourly/daily/monthly overruns, and — when the billing snapshot
carries a credit balance — the credit runway in days. `budget_exceeded` is
true when the hourly or daily overrun is positive, or the end-of-month
projection exceeds `monthly_usd`.

An exceeded budget blocks exactly one thing: **new cloud placement**. The
cycle logs the guard with all three overruns and gives the placement stage
zero new-cloud headroom. Existing work keeps running; reconciliation, advice,
and reporting continue. Independently of the guard, only `enforce-owned` mode
ever receives a nonzero new-cloud budget — `report` and `enforce-safe` place
no new cloud spend regardless of headroom.

## Reading workload cost: `stado cost`

Per-job and per-batch cost reporting from observed wall-times:

| Command | Answers |
|---|---|
| `stado cost report` | $ spent per target_kind and per model, from completed jobs. |
| `stado cost estimate <BATCH_FILE>` | Projected total $ for a batch file, using observed per-job cost. |
| `stado cost allocation` | The attributed provider/owner/workload cost ledger. |
| `stado cost forecast` | Current burn, month-end projection, budget, credit runway. |
| `stado cost anomalies` | Active cost and resource anomalies. |
| `stado cost savings` | Predicted versus realized savings, from measured decision outcomes. |

The reports behind these are persisted by the autonomy cycle under
`state/autonomy/cost/` (`prices.json`, `allocation.json`, `forecast.json`,
`anomalies.json`, `savings.json`), so they are readable even between cycles.

## Reading provider billing: `stado billing`

Cross-cloud costs, grants, burn, and credit balances, published as the
billing snapshot at `billing_health/credits.json`:

```bash
stado billing show
stado billing refresh
stado billing watch --interval 5m
```

`show` reads the last snapshot the coordinator published; `refresh` queries
the billing providers now and publishes a fresh one. `watch` is a foreground
watchdog that polls, evaluates the credit balance AND account health, and
alerts on transitions — a section with no balance figure at all
(`no_credentials`, `error`) is a finding, not a gap. It is deliberately
runnable outside the cloud it monitors: a collector that dies with its
provider cannot warn you about that provider, which is exactly what happened
when the collector ran as a Cloud Function inside the project it measured.
`--once` evaluates a single poll and exits.

## The historical GCP project

Billing for the historical GCP project (`wisent-480400`) is detached on
purpose and stays detached; it is not a defect. A `403` naming billing —
`accountDisabled` on a GCS or provider API call — means some part of the
system still depends on that project, and the GCP dependency is the thing to
remove, not the billing to restore. The 2026-07-27 outage this caused, and
the local-only containment profile that followed, are recorded in the
incident report; the billing service principal now comes from Skarbiec and
nowhere else, so no fallback can quietly recreate the cross-cloud coupling.

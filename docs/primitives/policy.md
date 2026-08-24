# Policy

What is Stado allowed to do on its own, and who decides? The autonomy policy
is one versioned document that answers both questions, and every autonomous
mutation is authorized against it fail-closed.

## What it is

The policy (`stado-rs/src/autonomy/policy.rs`) carries a schema version, a
required `policy_version` string, a mode, the emergency-pause flag, and the
budgets, placement, idle, freshness, and safety blocks. The default policy is
`default-report-only` in `report` mode: a fleet that has configured nothing
gets a policy that mutates nothing.

Three modes:

| Mode | Meaning |
|---|---|
| `report` | Every mutation is denied; the plan is recorded instead of executed ([operations](../operations.md)). |
| `enforce-safe` | Reversible actions are allowed; destructive ones still require an explicit rule. |
| `enforce-owned` | Destructive actions are allowed only when a matching resource rule explicitly sets `allow_destructive`. |

Authorization is fail-closed, in order: read-only actions are always allowed;
the emergency pause denies everything else; `report` mode denies; an
incomplete inventory denies when `require_complete_inventory` is set; a
resource that is not owned or adopted is denied; a production or stateful
resource is protected unless a matching rule explicitly allows its mutation; a
cost estimate above `max_single_action_usd` is denied; and financial
commitments are always denied — they require an operator-approved immutable
plan, in every mode.

**Budgets** cap spend: `hourly_usd`, `daily_usd`, `monthly_usd`,
`max_single_action_usd`, `max_commitment_usd`, each optional and required to
be finite and non-negative.

**Safety limits** bound each tick: `max_actions_per_tick` and
`max_actions_per_provider` (must be positive), `max_deleted_bytes_per_tick`,
`max_concurrent_mutations`, `require_complete_inventory`,
`protect_production`, `protect_stateful`, the circuit breaker
(`circuit_breaker_failures`, `circuit_breaker_cooldown_seconds`), and
`decision_ttl_seconds`. Only a mutation that failed on a host feeds the
circuit breaker; refusals computed before any host command runs must not
starve the healthy repairs behind them ([operations](../operations.md)).

**Freshness windows** (`inventory_max_age_seconds`,
`pricing_max_age_seconds`) bound how old the evidence behind a decision may
be; both must be positive.

**Resource rules** match a resource by type, provider, account, region,
environment, and owner, and default to allowing nothing: `allow_reversible`,
`allow_destructive`, `allow_production_mutation`, and
`allow_stateful_mutation` are all false until written otherwise, and a rule
cannot allow a production or stateful mutation without allowing a mutation at
all.

## Who declares it

An operator, atomically and with a version expectation:

```bash
stado optimize policy show
stado optimize policy apply --file policy.json --expect-version <version>
```

`apply` validates the document, then compare-and-swaps it against
`--expect-version`. The first policy is created only if absent; replacing an
existing policy without `--expect-version` is refused with
`autonomy policy already exists; expected_version is required`
(`stado-rs/src/autonomy/storage.rs`). Two operators cannot silently overwrite
each other's policy.

The emergency pause is separate from the mode and overrides every mode:

```bash
stado optimize pause "reason"
stado optimize resume
```

## Who observes it

`stado optimize status` shows the mode, safety state, inventory freshness, and
latest decisions; `stado optimize explain <decision-id>` prints one immutable
decision (`stado-rs/src/cli/autonomy_cmd.rs`). Every `stado optimize run` and
scheduled autonomy tick loads the policy before acting, and enforcing modes
execute under the emergency pause, circuit breaker, action limit, and
per-service mutation lease ([operations](../operations.md)).

## Where it lives

`state/autonomy/policy.json` in the configured canonical backend, versioned.
Every autonomy object is rooted under `state/` because the object gateway
authorizes writes against the configured namespace prefix allowlist
([operations](../operations.md)).

## Commands

```bash
stado optimize status
stado optimize run
stado optimize policy show
stado optimize policy apply --file policy.json --expect-version <version>
stado optimize pause "reason"
stado optimize resume
```

Flag-by-flag detail lives in [cli](../cli.md); what the autonomy layer does
with an authorized action is the subject of [autonomy](../autonomy.md).

## Not to be confused with

- **The emergency pause.** The pause is control state, not policy: it denies
  autonomous mutation in every mode until resumed, and changing the mode does
  not clear it.
- **A resource rule alone.** A rule never grants by itself — the mode and the
  rule combine, and a destructive action needs both `enforce-owned` and an
  explicit `allow_destructive` rule.
- **A [grant](grant.md).** The policy bounds what Stado may do; a grant bounds
  what credentials a Stado process may read. Neither substitutes for the
  other.

# Lease

Two reconcilers decide to repair the same service at the same moment — what
stops them both from acting? A lease: time-bounded ownership of one mutation
subject, held by whoever wrote it first and taken over only after it expires.

## What it is

A placement lease is a JSON object naming a `subject_id`, the `decision_id`
that motivated the mutation, a unique `token`, the `holder`, `acquired_at`, and
`expires_at` computed from a caller-supplied TTL in seconds.

Acquisition is a two-step atomic protocol:

1. Create-if-absent. If no lease exists for the subject, the writer wins.
2. If a lease exists and is still active, acquisition returns nothing — the
   caller does not act. If it has expired, the caller attempts a
   compare-and-swap against the exact stored version it read; a conflict means
   someone else took over first, and again the caller does not act.

Release is token-gated: the lease is deleted only when the stored token matches
the one the holder was issued. A lease whose token changed under a holder is
reported, not silently released — the reconciler surfaces "mutation lease
ownership changed before release" as an error on that action.

## Who declares it

The mutating code itself, immediately before an owned mutation. The autonomy
service reconciler acquires a per-service lease with subject
`service:<host>:<unit>` and holder `service-reconciler`, TTL taken from the
policy's `decision_ttl_seconds`, before executing any repair; a subject whose
lease is held elsewhere is recorded as `lease_blocked` with the detail
"another reconciler owns this service mutation" and counted as blocked, not
failed. The Box dispatcher acquires a conditional object-store lease before
every provider mutation, with a unique owner and fencing token per invocation;
a crashed tick remains fenced until its owner lease expires.

## Who observes it

Competing writers, by construction — the object is the arbitration. Operators
see the effects in service reconciliation records: `stado optimize status`
prints the latest reconciliation report, where `lease_blocked` outcomes appear
alongside repairs. Leases are one of the bounds on enforcing autonomy modes,
together with the emergency pause, the circuit breaker, and
`max_actions_per_tick` — see [autonomy](../autonomy.md).

## Where it lives

Placement and service-mutation leases live under
`state/autonomy/leases/` in the canonical [object-store](object-store.md)
(rooted under `state/` so the object gateway's prefix allowlist authorizes the
write). The queue keeps its own `leases/...` and `locks/...` prefixes for
conditional ownership, expiry, generation, and fencing state.

## Commands

There is no lease CLI; leases are acquired and released by the code that
mutates. Observe their consequences:

```bash
stado optimize status
```

## Not to be confused with

- **The [job](job.md) claim** — queued-to-running is create-if-absent with one
  winner and no TTL; job liveness is proven by heartbeats, not by expiry and
  takeover.
- **A [grant](grant.md)** — a credential capability. A lease confers no
  authority; it only serializes writers who are already authorized.
- **A [policy](policy.md)** — declares what autonomy may do; the lease
  serializes who does it right now.

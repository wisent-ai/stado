# Stado — high-level architecture plan

The architecture of Stado, laid out in the reviewable sequence
[product-guidelines](https://github.com/wisent-ai/product-guidelines) defines:
product contract → release contract → first success → product behavior →
external integrations → executable journeys → contract evidence → reader
documentation. Each stage names what Stado owns today, the target contract,
and the gate that decides the stage is complete. Mechanism detail lives in
[architecture.md](architecture.md); this file is the plan above it.

## 1. Product contract (README)

**Owns.** One policy-controlled queue, one canonical compute-target registry,
and one answer about fleet state: what runs where, on whose authority, with
what evidence. Machines the operator owns or explicitly authorizes; no hosted
multi-tenant service is promised.

**Target.** The README resolves every reader decision for a
gateway-plus-operated-fleet product: queue semantics, registry membership,
release channel, and the exact supported platforms (`darwin-arm64`,
`linux-amd64`).

**Gate.** A reader can state what Stado owns versus what belongs to Skarbiec
(credentials), Brama (model access), and Probierz (test evidence) without
reading source.

## 2. Release contract

**Owns.** The stado-native release pipeline: `release submit` builds from a
pinned host per platform, qualifies, signs (Ed25519 keys declared in
`release_control.trusted_keys`), publishes immutable archives into the
canonical object namespace, and pins per-target deliveries; `release status`
is the one progress answer (CLI and GUI render the same JSON);
`release install-local` is the delivery contract's local endpoint; provenance
is per-artifact (`stado host provenance`).

**Target.** Every managed binary on every registry host is reachable from a
commit on `origin/main`, every delivery is pinned to the host it runs on, and
a failed leg carries its own evidence (redacted command output, cursor into
the job log). Rollback is `release rollback` against a stored generation,
never a hand-copied binary.

**Gate.** `stado release status` explains any stalled rollout to the exact
failing leg without ssh; `stado host software` reports zero unmanaged
binaries across the fleet.

## 3. First success (onboarding)

**Owns.** `docs/onboarding.md`: exact-version install, `config init` →
`doctor` → one completed local job, with no cloud account. Fleet onboarding:
`registry host add` → `bootstrap` → Skarbiec grants → `host recover` →
the host reports in `registry beacon-age`.

**Target.** One command sequence takes a new machine from nothing to a
beaconing, release-managed registry target; the sequence is the checked-in
`docs/examples/fleet/onboard-host.sh` and stays executable as written.

**Gate.** A machine onboarded strictly by the script appears in
`registry beacon-age`, receives a pinned release, and completes one queue job.

## 4. Product behavior (core)

**Owns.** Verticals, one per user outcome: submit-and-collect (queue →
provider adapter → agent → canonical status/output objects); declare-and-
converge (registry v2 as the single truth; `registry doctor` diffs
declarations against live state; reconciliation through `optimize` and
declared host gates); resolve-and-route (`stado://service/<name>` through the
loopback resolver, authority snapshots, epoch-guarded placement moves);
observe-and-recover (beacons, capacity publications, disk-pressure gates,
`host reclaim` in declared janitor stages).

**Target.** Every host claim is the agent's own published word, never the
scheduler's inference; every mutation travels a compare-and-swapped registry
commit; failure states are first-class (`no_capacity_publication`,
`disk_pressure_unresolved`, `queue_paused`) and visible identically in CLI
and GUI.

**Gate.** For any host the fleet can answer, from records alone: why is this
host claiming nothing — with the blocker named, aged, and attributable.

## 5. External integrations

**Owns.** Skarbiec: scoped grants only, no raw vault items in Stado; secrets
never on remote command lines. Brama: the only model-access path for
workloads; Stado carries no provider model credentials. Probierz: quality
evidence rides Stado capacity through the bridge, evidence returns through
the configured object store. Weles: browser work placed on Stado-selected
hosts. Cloudflare/Tailscale: ingress and tailnet transport under
registry-declared identities.

**Target.** Each integration has an adapter boundary with its own failure
isolation: a dead neighbor degrades one capability and names itself; nothing
in Stado impersonates a neighbor's authority. No GCP dependency anywhere in
model access or scheduling.

**Gate.** `stado doctor` and `blast-radius` enumerate every external
dependency with live auth state; unplugging one integration fails only its
own verticals, with the integration named in the error.

## 6. Executable journeys (examples)

**Owns.** `docs/examples/`: onboarding-local-job.sh, fleet/onboard-host.sh.

**Target.** Every supported outcome has one canonical, checked-in, runnable
example: submit-and-collect, fleet onboarding, release rollout + rollback,
placement move, disk reclaim, service adoption. Examples state whether they
are read-only, mutating, or destructive.

**Gate.** Each example runs as written against a real registry target and its
claimed end state is observable through a read-only command.

## 7. Contract evidence (testing)

**Owns.** Rust test suites beside the code; Probierz owns run selection,
execution, evidence capture, and gate verdicts for behavioral claims —
Stado never certifies itself with ad-hoc test invocations.

**Target.** Observable contracts (registry compare-and-swap, release
qualification, resolver epoch rules, reclaim stage safety) are defended at
their boundaries; release promotion consumes Probierz gate verdicts bound to
the exact source revision.

**Gate.** A release cannot reach `Completed` without its platform legs, and a
behavioral claim in docs maps to a defended contract or a Probierz journey.

## 8. Reader documentation

**Owns.** `stado.wisent.com/docs` — Overview, Releases, Status & progress,
Hosts & disk — rendered by the shared `DocumentationLayout`, content in
`stado-landing/src/lib/docs.ts`, per
[documentation-guidelines](https://github.com/wisent-ai/product-guidelines/blob/main/documentation-guidelines.md).

**Target.** Docs change in the same commit series as the contract they
describe; every claim is demonstrable by a command the page shows.

**Gate.** `stado release status`, `stado host gates`, and `stado host
reclaim` behave exactly as their doc pages state, and the portal entry stays
"Dedicated site".

## Dependency order

A change to stage N reopens every stage after it: a new promise (1) reopens
the release story (2) and onboarding (3); a new mechanism (4) reopens
integrations it touches (5), its example (6), its evidence (7), and its doc
page (8). The guidelines' completion gates are the review checklist; this
file is the map of which gate guards which part of Stado.

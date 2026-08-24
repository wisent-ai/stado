# What is Stado

What is Stado, and what is the mental model for reading everything else in
these docs? Stado is a provider-neutral queue and fleet control plane: you
describe a workload, its required capacity, deadline, data, and budget; Stado
decides where and when to run it, then records what happened. The whole
product is three moving parts — a registry that declares, beacons that
observe, and reconciliation loops that close the gap between the two.

## The registry declares

The registry is the fleet's declaration of what SHOULD be true. It carries,
per host and per service:

- `targets[]` — every registered machine: its SSH channel, GPU label, VRAM,
  and slot cap. A target may also carry `managed_versions`, the version each
  stado-managed binary is required to be at on that host. The field is
  optional on purpose: a target that declares nothing is reported as
  `undeclared`, never as agreeing.
- `targets[].services[]` — the launchd/systemd unit, program, arguments, and
  host-side unit path Stado is required to keep running.
- `service_directory` — the fleet routing contract. `authority` names the one
  target and binary allowed to serve canonical routing snapshots and commit
  placement changes; `generation` is the monotonic routing epoch; each
  service declares its active host, per-host endpoint, and the exact
  consumers and capabilities allowed to resolve it.

Workloads address `stado://service/<name>`, never a host or port. The
declaration lives in versioned, generation-fenced registry state; see
[configuration](configuration.md) for the document itself and
[architecture](architecture.md) for how resolution works.

## Host beacons observe

Every registered host publishes a health beacon — local disk and service
state — on a one-minute tick: a systemd timer on Linux, a LaunchAgent on
macOS. The beacon prefix holds the latest document per host, and answers that
cost no SSH: `stado service list` reads unit state for the whole fleet from
beacons alone, including hosts that are not currently reachable.

Silence has one fleet-wide threshold, five minutes by default, because the
writers tick once a minute: three consecutive misses is a host that has
stopped talking, one miss is a slow local call. A beacon older than the
threshold cannot describe the present, so every answer derived from it
becomes `unknown` — deliberately not the same answer as `missing`, and never
grounds for a host mutation. Each silence gap is recorded durably under
`state/host_silence/`, together with the readers that refused while the host
was quiet, so an outage that closes over itself still leaves evidence.

## Reconciliation closes the gap

`stado host inventory <target>` is the observation that reconciles declared
against actual state, along axes that are deliberately independent:

| Axis | Question |
|---|---|
| Marker vs listener | Is anything listening where the host's forward marker points? |
| Marker vs registry | Does that marker point where `service_directory` declares? |
| Binary vs registry | Is the installed binary at the `managed_versions` version? |
| Service unit vs beacon | Does a fresh host beacon report the declared unit? |
| Service endpoint vs consumers | Does the declared address answer from its required vantages? |

Keeping the axes apart is the point: a marker can match the socket table and
disagree with the registry at the same time, and collapsing the two into one
"drift" verdict hides exactly that case. The full state tables are in
[architecture](architecture.md).

The autonomy loop reconciles the last two axes together. During every
`stado optimize run` and scheduled autonomy tick, the coordinator joins two
independent facts — the unit state in the newest host beacon and a fresh
`stado service verify` reachability sweep — and writes the result as a
durable report. A stale beacon is `unknown`, never `missing`. A fresh missing
unit plus a proven-unreachable endpoint enters the idempotent
`service ensure` repair; a responding endpoint is adopted only after Stado
proves a loaded unit owns the declared program. Report mode records the
action; enforcing modes execute it under the emergency pause, circuit
breaker, action limit, and per-service mutation lease. Detection precedes
automatic delivery, in that order and on purpose. See
[operations](operations.md) and [autonomy](autonomy.md).

## What Stado is not

Stado's canonical object store holds job records, leases, capacity
broadcasts, control state, results, artifact manifests, and recovery
metadata — storage is authoritative, and provider APIs and dashboards are
observations, not alternate queues. The API listener serves control-plane
state only, no HTML page; the operator workspace is Stado Desktop. Stado
holds no secrets: Skarbiec is the authority for narrow credentials, workload
secrets resolve from Skarbiec at execution time, and secret plaintext is
materialized only inside the trusted workload process, excluded from durable
job JSON. And Stado routes no model requests: it declares inference
deployments and desired routes in the registry, but Brama — the gateway on
the declared `gateway_target` — is what serves requests, reloading the
committed route snapshot Stado stages.

## The first three commands

```bash
stado overview
```

One operator snapshot: jobs, active workers, quota, budgets, burn, and
credits. The answer to "what is the fleet doing right now".

```bash
stado service list
```

Every registry-managed service across all hosts, with its state — answered
from the latest health beacons, so it costs no SSH and reports on hosts that
are not currently reachable. `STATE` is what the host says about its own
unit; `OBSERVED` is when anybody last checked the service from outside.

```bash
stado submit "printf 'hello from Stado\n'"
```

Submit a job to the queue. It prints a `Job ID`; `stado status` and
`stado results` answer what happened to it. The end-to-end path is
[quick-start](quick-start.md); the full command surface is [cli](cli.md).

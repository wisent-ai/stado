# Core use cases

One section per thing an operator actually does with Stado: who acts, what
they run, and what the product records afterwards.


## Run a workload on an existing machine

- **Actor:** an operator and a workload owner.
- **Initial state:** the operator has registered a workstation or server and
  started an agent with the required runtime and capacity.
- **Outcome:** the owner submits a command with optional CPU, GPU, deadline,
  artifact, and verification constraints; Stado leases it to one eligible agent,
  records every transition, and returns output by job ID.
- **Safety boundary:** the workload receives only admitted capacity and named
  secret references; host registration does not grant general provider access.

## Share one queue across a fleet

- **Actor:** an infrastructure operator managing multiple authorized agents.
- **Initial state:** agents publish capacity to one configured canonical store.
- **Outcome:** eligible workers claim work from the same queue while operators
  see one job lifecycle and result contract.
- **Safety boundary:** leases, fencing, and compare-and-swap revisions prevent
  two workers or coordinators from owning the same transition.

## Pause and drain safely

- **Actor:** an operator preparing maintenance or migration.
- **Initial state:** queued and running jobs may exist across the fleet.
- **Outcome:** the operator pauses new claims and dispatches, waits for running
  jobs to finish or yield, verifies drain state, and resumes without deleting
  queued work.
- **Safety boundary:** pause and drain are durable control state, not a best-
  effort process signal on one machine.

## Recover from a storage outage

- **Actor:** an operator responsible for the canonical queue and artifact store.
- **Initial state:** the active local, GCS, S3, or Azure Blob backend is degraded
  or must be replaced.
- **Outcome:** the operator previews and executes a fenced copy, verifies names,
  metadata, and bodies, then selects the recovered canonical store.
- **Safety boundary:** migration does not allow two active writers and does not
  silently treat an unavailable backend as an empty queue.

## Run with reproducible inputs and bounded secrets

- **Actor:** a workload owner submitting repeatable AI work.
- **Initial state:** source, immutable inputs, requested secret fields,
  postcondition, and output contract are explicit.
- **Outcome:** the worker resolves those inputs, executes the workload, and
  publishes output plus SHA-256 evidence.
- **Safety boundary:** secret plaintext is materialized only inside the trusted
  workload process and is excluded from durable job JSON.

## Give automation safe compute access

- **Actor:** an external service or AI agent.
- **Initial state:** the caller has credentials for the exact Stado interface and
  action it needs.
- **Outcome:** it uses versioned `stado machine` JSON for authorized mutations
  and status, or read-only `stado-mcp` for inspection, and receives stable
  machine-readable errors.
- **Safety boundary:** neither interface provides unrestricted shell access or
  cloud-administrator credentials.

## Burst to an explicitly enabled cloud provider

- **Actor:** an operator-controlled workload workflow.
- **Initial state:** identity, network, quota, image, ownership, cost, recovery,
  and provider-specific capability boundaries are configured.
- **Outcome:** Stado may provision an eligible VM, bootstrap an agent, execute
  the same job contract, collect the result, and retire the owned instance.
- **Safety boundary:** each cloud adapter remains preview until its live
  acceptance suite is recorded for the released version; missing evidence is not
  promoted to stable support.

## Host a web product on a public hostname

- **Actor:** an operator replacing a third-party build-and-host platform.
- **Initial state:** the product's repository carries a `.wisent-release.json`
  with a `web` platform, and the fleet carries a declared public edge host.
- **Outcome:** `stado release submit` builds the product on a fleet builder and
  publishes one runnable tarball; `stado web deploy` installs it as a managed
  unit with its environment delivered field by field from Skarbiec and its
  database credential resolved for that unit's own consumer; `stado web route`
  puts a certificate on the edge and then writes the hostname's DNS record
  through `stado dns`.
- **Safety boundary:** the unit binds loopback and the edge owns 443; a
  hostname's record is written only after its certificate exists; no
  credential value is read by the operator or placed in a command line; and a
  product that is not a declared consumer of a database cannot receive its
  credential. The contract is [Web hosting](https://stado.wisent.com/docs/web-hosting).


# Integration Contracts

This document is the release boundary for Stado integrations. The machine-readable adapter status remains `stado capabilities --json`; this document adds lifecycle, ownership, outage, and promotion rules. An adapter being implemented does not make it stable. Stable support requires release-scoped live acceptance evidence.

## Status matrix

| Integration | Capability status | Stado 0.5 boundary | Promotion evidence |
|---|---|---|---|
| Local compute | implemented execution; partial provisioning | Core path. An operator attaches an existing host; Stado does not provision the machine. | clean install, first workload, cancellation, restart recovery, upgrade, rollback |
| Local filesystem storage | implemented | Core single-host and recovery backend. Not a shared distributed filesystem contract. | atomic claim, CAS, listing, copy, outage, restart recovery |
| Skarbiec | implemented | Canonical optional secret resolver. Required only for workloads or routes that declare secret references. | scoped grant, missing/expired grant, outage, redaction, revocation |
| Google Cloud Storage | implemented adapter | Preview object-store adapter; excluded from stable 0.5 support until its live sandbox suite passes. | metadata/CAS, pagination, copy, delete, outage, recovery |
| Amazon S3 | implemented adapter | Preview object-store adapter; excluded from stable 0.5 support until its live sandbox suite passes. | metadata/CAS, pagination, copy, delete, outage, recovery |
| Azure Blob Storage | implemented adapter | Preview object-store adapter; excluded from stable 0.5 support until its live sandbox suite passes. | metadata/CAS, pagination, copy, delete, outage, recovery |
| Google Compute Engine | partial | Preview VM lifecycle only. Never promoted by unit or mocked API evidence. | real owned VM create, bootstrap, workload, collect, cancel, reap, billing and quota observation |
| AWS EC2 | implemented VM adapter | Preview VM lifecycle; EC2 Auto Scaling managed compute is planned and unavailable. | live release-scoped VM lifecycle suite before stable support |
| Azure Virtual Machines | implemented VM adapter | Preview VM lifecycle; VM Scale Sets managed compute is planned and unavailable. | live release-scoped VM lifecycle suite before stable support |
| Box | implemented fixed-shape lifecycle | Preview leased CPU sandbox with durable ownership and fencing. It is not a general autoscaling VM provider. | allocate through release, crash recovery, lease expiry, output persistence |
| Vast.ai | partial | Execution on an operator-published host only. Renter provisioning and marketplace autoscaling are unavailable. | published-host execution, cancellation, disappearance, output recovery |
| Alerts | optional adapters | Slack, Telegram, SendGrid, Pub/Sub, and billing alerts are not a queue, execution, or health dependency. | route-specific delivery and provider-outage isolation |

`planned` means no callable Stado contract. In particular, AWS Auto Scaling and Azure VM Scale Sets must not be inferred from the EC2 and Azure VM lifecycle adapters.

## Common adapter contract

Every enabled integration must define all of the following before use:

- **Capabilities:** exact implemented, partial, external, planned, and unsupported operations.
- **Enablement:** an explicit deployment profile and provider fence. Presence of ambient credentials never enables an adapter.
- **Authentication:** one provider-scoped identity. Workloads never inherit control-plane credentials.
- **Data ownership:** canonical object prefixes or provider ownership labels; foreign resources are read-only.
- **Outage behavior:** fail closed at the adapter boundary. No silent provider, bucket, credential, or backup-store fallback.
- **Observability:** bounded, secret-free error codes and provider references; no token, command environment, or object body logging.
- **Lifecycle:** create or attach, observe, mutate, recover, and release behavior with idempotent retries.
- **Cleanup:** deletion only after durable terminal state and artifact evidence; uncertain ownership blocks deletion.

## Compute lifecycles

### Local compute

1. Register an existing host and explicit SSH or local install destination.
2. Install one checksum-verified immutable release and start the native agent.
3. Publish native RAM, disk, GPU, VRAM, and slot capacity.
4. Claim a queued job atomically, write `running/`, then execute the declared workload.
5. Upload redacted output before the terminal state transition.
6. On cancellation, terminate the process group, retain durable cancellation evidence, and release the claim.
7. On restart, reconcile durable running state; never create a second owner for the same claim.

Local compute owns workload processes and Stado state. It does not own host provisioning, the operating system, network policy, GPU drivers, or workload-specific runtimes.

### Cloud VM adapters

1. Preflight explicit provider enablement, scoped identity, network, image, quota, release, ownership, and cost policy.
2. Persist the allocation intent and fencing identity before the provider mutation.
3. Create only a Stado-owned VM and bootstrap an exact checksum-verified release.
4. Observe readiness before dispatch; a provider reference alone is not readiness.
5. Execute the provider-neutral job contract and persist output before terminal state.
6. Reconcile provider and Stado state after retries or coordinator restart.
7. Cancel or reap only a resource whose ownership labels and expected reference match.
8. Delete paid capacity after durable completion, cancellation, or explicit recovery disposition.

All cloud VM adapters remain preview. AWS and Azure VM adapters do not imply managed-group support.

### Box

The durable Box lifecycle is `allocating -> provisioning -> ready -> starting -> running -> collecting -> releasing -> released`, plus recoverable failure. Every provider mutation is protected by a conditional object-store lease, unique owner, and fencing token. Output must be durable before archive or deletion. A coordinator restart resumes from recorded state rather than launching a duplicate command.

### Vast.ai

Stado may publish and operate an agent on an explicitly configured host. It may observe marketplace state used by that published host. It does not rent arbitrary capacity, choose offers, or promise marketplace autoscaling. Host disappearance is an execution failure to reconcile, not permission to create replacement capacity.

## Storage lifecycle

All four storage adapters implement the same `BlobBackend` and `JobStorage` contract:

1. Bind exactly one configured backend as canonical and optionally one explicit recovery destination.
2. Validate the versioned storage-layout marker before mutation.
3. Create claims, locks, and immutable objects conditionally; use backend version tokens for compare-and-swap updates.
4. Preserve object metadata used for versioning, checksums, pagination, and recovery.
5. Treat listing cursors as opaque and enumerate the complete requested prefix.
6. Make delete idempotent and surface authorization, precondition, throttling, and transport failures.
7. During migration: pause, drain, fenced copy, verify names/metadata/bodies, then explicitly cut over.
8. Never write automatically to the backup because the primary is unavailable.

The local backend provides process-safe filesystem semantics on one host. It is not safe to reinterpret unrelated shared-filesystem behavior as this contract. GCS, S3, and Azure Blob use their provider-native conditional generation/version primitives; stable support is withheld until the corresponding live sandbox suite passes for the release candidate.

## Skarbiec lifecycle

1. A trusted route or workload declares an allowlisted item and field, never plaintext.
2. The caller receives a dedicated consumer grant scoped to only those actions and items.
3. Plaintext resolves at execution time into process memory and the workload environment.
4. Job JSON, machine responses, status errors, logs, result uploads, and artifact manifests remain plaintext-free.
5. Missing, expired, revoked, or unreachable authorization fails only the requesting operation and does not broaden scope or fall back to another credential source.
6. Grants are rotated or revoked independently of immutable releases and persisted workload metadata.

## Optional integrations

Alert sinks are leaf integrations. Disabling every alert sink must leave submit, scheduling, execution, storage recovery, local listener operation, and machine authentication functional. A sink's verifier or network outage is reported on the affected route and must not stall the coordinator or agent loop.

Alert payloads contain bounded operational identifiers and classifications, not workload output or secret material.

## Release gate

Each integration is promoted independently. Required evidence exercises a real provider boundary, not only a mock. A release note records the tested account or sandbox class, platform, operation set, outage behavior, cleanup result, and known limitations. Failure of one optional integration blocks only that integration's promotion unless it violates the shared queue, security, or recovery contract.

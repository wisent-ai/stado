# Examples

Where is the runnable material, and what does each piece prove? This page is
the index: every entry links to a script or scenario that exists in the
repository, with what it runs and what success looks like.

The rule for everything listed here: an example that cannot be pasted and run
does not belong here. Per the examples
[README](examples/README.md), each script is the plain command sequence a user
would type, in order — `set -eu`, a usage comment, env for values, nothing
else — and verification is itself a printed command.

## Core scripts

Four scripts in [docs/examples/](examples/README.md) cover the first-run,
work, secrets, and queue surfaces.

### [onboarding-local-job.sh](examples/onboarding-local-job.sh)

From zero to one completed local job, no cloud account needed. Runs
`stado config init`, `config validate`, `doctor --fix-hints`, then submits a
trivial local workload (`stado submit --profile local -- echo
hello-from-stado`), watches it with `stado job watch`, and downloads the
result with `stado results`. Success is the watched job finishing and
`results` printing the downloaded output of that job.

```bash
sh onboarding-local-job.sh
```

### [secrets-store-and-read.sh](examples/secrets-store-and-read.sh)

The daily secrets loop. The value travels via stdin only, because argv leaks
into `ps` and shell history. Runs `stado secrets put demo-vendor` fed by
`printf`, reads it back with `stado secrets get demo-vendor`, then lists what
the grant may see with `stado secrets ls`. Success is `get` printing the
stored value and `ls` showing `demo-vendor`.

```bash
EXAMPLE_SECRET=... sh secrets-store-and-read.sh
```

### [queue-maintenance.sh](examples/queue-maintenance.sh)

Stop dispatch, let running work finish, reopen — nothing is cancelled, queued
jobs wait for resume. Runs `stado queue status`, `pause`, `status` again,
`drain`, `resume`, and a final `status`. Success is the final `queue status`
showing the queue open again with no work lost.

```bash
sh queue-maintenance.sh
```

### [fleet-health-check.sh](examples/fleet-health-check.sh)

Fleet truth without ssh: who reported lately, who answers, what services run.
Runs `stado registry beacon-age` (every registry host and its last heartbeat,
worst first), `stado host ping <target> --json` (reachability verdict — ssh
check plus beacon age — one target per invocation), and
`stado service list` (managed services across the fleet, from beacons alone).
Success is all three commands printing fleet state gathered from beacons and
the registry, with no direct login to any machine.

```bash
sh fleet-health-check.sh
```

## Fleet scripts

The [fleet/](examples/fleet/add-remove-host.sh) subdirectory holds the
enrollment sequences, one method per script:

- [add-remove-host.sh](examples/fleet/add-remove-host.sh) — the `declare`
  method: `stado registry host add` with `--ssh` and `--release-platform`,
  removal via pull → edit → validate → push; ends net-zero, the registry
  looks exactly like before.
- [onboard-host.sh](examples/fleet/onboard-host.sh) — bring a device to
  reporting life over a channel that already exists, after the machine
  trusts the fleet's public key; only the public key ever reaches the
  machine.
- [invite-a-machine.sh](examples/fleet/invite-a-machine.sh) — the `invite`
  method end to end, operator side, in the offline mode that publishes
  nothing: the operator never touches the machine and the fleet's private
  key never leaves the operator's vault.

## Provider scripts

The [providers/](examples/providers/enable-aws.sh) subdirectory lights up
opt-in backends, per user. Each follows the same shape: credentials from your
env into your skarbiec, provider flipped on in your config, one verify
command.

- [enable-aws.sh](examples/providers/enable-aws.sh) — `stado-aws` credentials
  from `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, verified with
  `stado config validate`.
- [enable-azure.sh](examples/providers/enable-azure.sh) — the
  `wisent-azure-billing-sp` service principal from `AZURE_TENANT_ID`,
  `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`.
- [enable-gcp.sh](examples/providers/enable-gcp.sh) — `stado-gcp` service
  account JSON from `GCP_SERVICE_ACCOUNT_JSON`, verified with `stado doctor`.
- [enable-vast.sh](examples/providers/enable-vast.sh) — `stado-vast` API key
  from `VAST_API_KEY`, verified with `stado vast list`.

## Fleet scenarios

The [fleet examples](../examples/fleet.md) at the repository root are prose
scenarios with command sequences rather than single scripts:

- **Onboarding a machine** — agent-attested enrollment: a join request or
  `enroll` creates only a non-routable `provisioning_targets` entry, and the
  machine enters `targets` and a fleet only after the installed agent
  publishes fresh capacity with its Stado version.
- **Fleets as named sets** — declaring fleets in the canonical registry,
  assigning machines (one fleet per machine), and inspecting them with
  `list`, `status`, and `doctor --fleet`.
- **Keys and the catalog** — SSH host keys in the same global credential
  backend as every other Stado credential, and the central enrollment
  catalog every machine can read.

## Walkthroughs

Two scenarios are written as full prose walkthroughs rather than scripts:

- [service deploy plus self-repair](walkthrough-service-repair.md) — one
  declared service breaking and the autonomy loop repairing it, as commands
  and their readings.
- [release end-to-end](walkthrough-release.md) — one release from source to
  fleet and back, including rollback and per-host quarantine.

For flag-by-flag command details, see the [cli](cli.md) reference.

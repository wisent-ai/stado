# Onboarding

This path takes a new operator from no Stado state to one completed local job. Local mode needs no cloud account, provider CLI, or Wisent production credential.

## Install an exact release

Set the immutable release identity, run the verified installer, and expose its binary directory:

```bash
export STADO_RELEASE_API_URL=https://stado.wisent.com
export STADO_RELEASE_VERSION=<exact-version>
export STADO_RELEASE_PLATFORM=<exact-platform>
./install-stado.sh
export PATH="$HOME/.stado/bin:$PATH"
```

Expected result: the installer prints `installed Stado <version> for <platform> in <directory>`. It rejects a manifest identity mismatch, missing artifact, digest mismatch, or checksum-list mismatch before replacing a binary.

Running `stado` without arguments prints the first-run path and exits successfully. It does not create config or mutable state.

## Create the minimum local configuration

```bash
stado config init
stado config validate
stado doctor --fix-hints
```

Expected results:

- `config init` prints the path to the new `$HOME/.stado/config.json`;
- the file selects only the local compute provider, local primary and backup storage, a local deployment identity, and loopback dashboard binding;
- `config validate` prints `config ok (<path>)`;
- `doctor --fix-hints` reports actionable missing workload dependencies without requiring cloud credentials.

`config init` never overwrites an existing file. For a legacy config without a schema marker, use `stado config migrate`; it preserves the exact prior document as a timestamped sibling before adding the current schema. Production Wisent policy, verifier allowlists, and cloud credentials belong in explicit deployment profiles under `deploy/`, not in first-run config.

## Start the control plane

```bash
stado local-control-plane
```

Expected result: the process starts the queue coordinator, local worker, and loopback dashboard. Leave it running. Open the loopback address printed by the process from the same machine; do not expose it publicly without the documented deployment authorization boundary.

## Submit and inspect one job

From another terminal using the same config:

```bash
stado submit "printf 'hello from Stado\n'"
stado status <job-id>
stado results <job-id> ./stado-result
```

Expected result: status progresses from queued or running to completed. The downloaded command output contains `hello from Stado`; the result manifest records the artifact size and SHA-256. A failed job remains inspectable and may still publish logs and partial artifacts.

## Onboard another machine

The coordinator must be able to reach the host through an explicit SSH
destination. Enrollment is one transaction: Stado stages a non-routable
entry, installs the agent, waits for fresh capacity, and only then registers
the target:

```bash
stado_fleet enroll <target-name> --ssh <user@host>
stado registry doctor
```

SSH reachability alone is not registration. Installation failure or missing
agent attestation rolls the staging entry back. To adopt the current machine
from a legacy registry entry without SSH, run
`stado_fleet reconcile <target-name>` on that machine.

## Failure guidance

- `config file already exists`: validate or migrate it; do not overwrite it implicitly.
- `ERROR config schema_version ...`: run `stado config migrate` only for a trusted legacy config. Future schema versions require a compatible Stado release.
- storage unreachable: stop submission, preserve the source, and follow `stado storage backup`, `stado storage verify`, or the outage recovery procedure. Unreachable storage is never treated as an empty queue.
- job remains queued: confirm an eligible worker is running, the queue is not paused, capacity fits, and the workload deadline has not expired.
- worker rejects or fails a job: install the requested shell/runtime/driver on that worker or change the workload. Stado does not silently supply workload dependencies.
- dashboard authorization error: use the deployment-bound operator credential. Do not bypass or disable the authorization boundary.
- immutable release collision or digest failure: stop. Publish a new version; never overwrite the existing coordinate.

## Uninstall and local reset

The uninstall script requires an explicit confirmation value and preserves config and queue data by default:

```bash
STADO_UNINSTALL_CONFIRM=uninstall-stado ./uninstall-stado.sh
```

It disables Stado launchd/systemd-user services, removes installed Stado binaries, and leaves `$HOME/.stado/config.json`, local storage, and local backup intact.

To remove those local data stores and config as well:

```bash
STADO_UNINSTALL_CONFIRM=uninstall-stado ./uninstall-stado.sh --purge-data
```

`--purge-data` is irreversible. Before using it, stop all writers and copy any queue, results, artifacts, or configuration that must survive. It does not delete cloud objects or credentials outside the local Stado paths.

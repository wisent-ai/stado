# Onboarding

This path takes a new operator from no Stado state to one completed local job. Local mode needs no cloud account, provider CLI, or Wisent production credential.

## Install an exact release

Set the immutable release identity, run the verified installer, and expose its binary directory:

```bash
export STADO_API_URL=https://stado.wisent.com
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
- the file selects only the local compute provider, local primary and backup storage, no remote deployment binding, and loopback dashboard binding;
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

Enrollment is verified, not declared: every write is preceded by a probe over Stado's own SSH channel, and that channel uses only the target-scoped key in the selected credential store. So the machine needs three things before the control plane can say anything true about it — a reachable SSH destination, Remote Login enabled, and the target's public key in its `authorized_keys`.

### Any reachable destination counts

The registry stores the SSH destination verbatim and requires no particular kind of address. `user@machine.local` on the same LAN is as valid a target as a tailnet name or a routable host; enrollment probes whatever you give it and records the machine it actually reached.

A `.local` destination costs reach, not correctness. Every command that opens the channel — `stado fleet enroll`, `stado fleet key check`, `stado host recover`, `stado host exec`, `stado bootstrap` — then works only from inside that network and fails with an unreachable destination from anywhere else. The health beacon travels the other way: the host publishes it outward itself, so `stado registry beacon-age` and `stado host health <target>` keep reporting that machine from anywhere, including while its channel is out of reach. Registering a machine by its `.local` name is therefore a complete way to attach it and watch it, and an incomplete way to administer it remotely.

### Verified enrollment

Generate the key first, because that is what prints the public half:

```bash
stado fleet key generate <target-name>
```

Append the printed line to `~/.ssh/authorized_keys` on the machine being added. This is the one step that happens on that machine: there is no channel yet, so `stado fleet key install`, which appends the key *through* the channel, is a rotation tool and not first contact. `stado fleet key generate` leaves the stored key readable to the local operator itself; onboarding never sends you to a repository script. If the machine belongs to someone else, hand them [Add your own machine](add-your-machine.md), which covers only their two steps.

Then, from the control plane:

```bash
stado fleet enroll <target-name> --ssh <user@host> --bootstrap
stado fleet key check <target-name>
stado host recover <target-name>
stado registry beacon-age
```

`enroll` probes `hostname`, `uname -s` and `uname -m` before writing, so the entry carries the machine's real hostname and the release platform it actually is; `--bootstrap` installs the agent and rolls the entry back if that fails. An unverifiable or uninstallable machine never stays in the registry. `--kind` defaults to `local`, and `--fleet <name>` places the machine in a declared fleet in the same call. `stado fleet key check` proves the channel, `host recover` installs the health beacon and the managed units, and `beacon-age` is the proof — a target with no beacon at all is listed, never omitted.

A reporting host also needs its two Skarbiec grants, which the control plane mints:

```bash
skarbiec token-mint stado-local-agent --scopes 'read:*'
skarbiec token-mint stado-host-health-beacon --scopes 'read:stado-host-health-api'
```

When the control plane cannot reach the machine but the machine can reach the store, the machine announces itself instead: `stado fleet join` on it, then `stado fleet pending` and `stado fleet approve <hostname>` here; `stado fleet reject <hostname>` drops a request. Both paths honour the registry's optional `enrollment` catalog, printed by `stado fleet catalog`.

Stado Desktop offers the same enrollment without a terminal: **Fleet › Hosts**, the **Add a Machine** action, then one sheet that names the machine, shows the generated public key with the exact `authorized_keys` line to paste, takes the SSH destination, and runs the same probe-then-write enrollment before the machine appears in the Hosts table. It issues the `stado fleet …` commands documented here through the dashboard's authenticated command bridge rather than carrying its own enrollment logic, so the CLI remains the canonical surface and the two cannot disagree. The `stado_fleet` binary still exists for compatibility over the same implementation; new instructions should use `stado fleet`.

### The declaration alone

The lower-level write skips the probe and records what you assert:

```bash
stado registry host add <target-name> --ssh <user@host> --release-platform <exact-platform>
stado registry doctor
stado bootstrap --target <target-name> --dry-run
stado bootstrap --target <target-name>
stado host health <target-name>
```

`--ssh` and `--release-platform` are both required; `--kind` defaults to `local`. Review the dry-run unit before installation. The worker host must already provide every runtime and driver its jobs require. Registry identity, SSH reachability, workload dependencies, and health publication are separate checks; passing one does not imply the others.

For the current machine, `stado bootstrap --local --target <target-name>` installs
the launchd or systemd-user unit directly.

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

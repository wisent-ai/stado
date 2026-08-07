# Fleet examples

A fleet is a named set of registry machines. Fleets are declared in the
canonical registry: an optional top-level `fleets` section holds the names
and notes, and each target points at its fleet with its own `fleet` field.
The section is additive — older readers ignore both keys, and a document
without it simply has no fleets.

All writes go through the validated compare-and-swap registry path, the
same one `stado registry push` uses, so a malformed change is refused
before it lands.

## Onboard a new machine

Enrollment is agent-attested: SSH reachability and hostname discovery create
only a non-routable `provisioning_targets` entry. The machine enters
`targets` and a fleet only after the installed agent publishes fresh capacity
with its Stado version.

**Machine-initiated request:**

```sh
# on the new machine:
stado_fleet join
# on the control plane:
stado_fleet pending
stado_fleet approve <hostname-from-the-request> \
  --ssh operator@render-node-a.local \
  --fleet render-burst
```

`join` records the machine's real hostname, OS and architecture in the
store. Approval still requires an installation channel: it probes that
machine, installs the canonical Stado agent, requires a fresh capacity
attestation matching the request hostname, and only then promotes the
request. `reject` drops a request without touching the registry.

**Control-plane-initiated:**

```sh
stado_fleet enroll render-node-a \
  --ssh operator@render-node-a.local \
  --fleet render-burst
```

`enroll` performs the same transaction without a preceding join request.
The agent installation is mandatory, not a flag. An unreachable host,
failed installation, missing attestation, or registry conflict removes the
provisioning entry and leaves no registered target. The SSH destination may
resolve over LAN, mDNS, or an overlay address such as a tailnet IP.

Legacy declarations are repaired through the same contract:

```sh
stado_fleet reconcile render-node-a
```

`reconcile` uses the target's declared SSH channel, withdraws the unverified
entry while provisioning, and restores fleet membership only after a live
agent attestation.
When the entry represents the current machine and has no SSH field,
`reconcile` uses the local installer after matching the machine's real
hostname. It never treats an arbitrary SSH-less entry as local.

After every legacy local target has a receipt, make the invariant global:

```sh
stado_fleet enforce-attestation
```

The command is atomic and fails with the first unreconciled target; it never
enables a policy that would invalidate the current registry.

## Declare a fleet

```sh
stado_fleet create render-burst --notes "Spot GPU capacity for rendering jobs"
```

Rules: the name is a lowercase identifier (letters, digits, dot, dash,
underscore). Duplicates are refused.

## Add machines to it

Machines are added by name, one fleet per machine; assigning again simply
moves the machine:

```sh
stado_fleet assign render-node-a render-burst
stado_fleet assign render-node-b render-burst
```

The target must carry a successful `agent_enrollment` attestation and the
fleet must already be declared. A target name, SSH probe, or join request
alone is insufficient.

## Inspect

```sh
stado_fleet list                  # every fleet with its members
stado_fleet list --json           # machine-readable, for automation
stado_fleet status render-burst   # live beacons and capacity of one fleet
stado_fleet doctor --fleet render-burst
```

`doctor --fleet` scopes the beacon and capacity checks to one fleet and
exits non-zero when any member cannot run; the credential-grant section
always covers the machine the command runs on.

## Current fleets on this deployment

- `core` — always-on machines that watch the herd
- `burst` — heavy GPU capacity for burst work
- `interactive` — operator workstations that may sleep

## SSH host keys in the credential store

Host keys use the same global backend as every other Stado credential.
`STADO_CREDENTIALS_STORE` requests the backend; `credentials.store` in the
config records the committed backend. There is no separate registry
`key_custody` switch and no fallback to `~/.ssh`.

```sh
stado_fleet key add render-node-a --from ~/.ssh/id_ed25519   # move; source removed after read-back
stado_fleet key generate render-node-b                        # generate into the selected store
stado_fleet key rotate render-node-a                          # safe end-to-end rotation
stado_fleet key ls                                            # metadata only
stado_fleet key install render-node-a                         # public key -> authorized_keys
stado_fleet key check render-node-a                           # verify the stored key
stado_fleet key rm render-node-a                              # delete from the selected store
```

Private material is never printed. One remote call materializes it into an
owner-only temporary file for `ssh -i` and removes that file immediately.
Rotation installs the new public key through the old stored key, replaces the
credential item, verifies the new key, then removes the old public key. Failed
verification restores the old item.

Changing backends moves SSH keys together with every other credential:

```sh
export STADO_CREDENTIALS_STORE=file:///secure/stado-credentials.json
stado secrets migrate
```

Until migration verifies and commits the new backend, every credential access
fails closed instead of falling through to a second location.

## The central catalog

Which registration paths the fleet allows, and how machines reach the
control plane, is declared once — in the canonical registry, readable by
every machine:

```json
{
  "enrollment": {
    "allow_join": true,
    "allow_enroll": true,
    "require_verified_hostname": true,
    "require_agent_attestation": true
  },
  "channels": {
    "control_plane": ["loopback"],
    "notes": "anything the address resolves over: LAN, mDNS, tailnet"
  }
}
```

`stado_fleet catalog` prints it. `join`, `approve` and `enroll` consult
the path policy before any write. Once `require_agent_attestation` is true,
the registry validator also rejects every local target without a valid
receipt. A document without the sections remains unrestricted for migration,
and `catalog` says so explicitly.

## Schema reference

What the commands above produce inside `registry.json`:

```json
{
  "fleets": [
    { "name": "render-burst", "notes": "Spot GPU capacity for rendering jobs" }
  ],
  "targets": [
    { "name": "render-node-a", "kind": "local", "fleet": "render-burst" }
  ]
}
```

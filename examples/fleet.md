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

Enrollment is verified: a machine lands in the registry only after Stado
proves it exists. Two channels, both verified:

**Machine-initiated (no address needed):**

```sh
# on the new machine (no arguments, no hostname needed):
stado_fleet join
# on the control plane:
stado_fleet pending
stado_fleet approve <hostname-from-the-request> --fleet render-burst
```

`join` records the machine's real hostname, OS and architecture in the
store and prints the request (for setups where the store is not shared,
the printed JSON travels by any channel). `approve` turns it into a
registered target through the validated registry write — a host identity
already declared is refused, never duplicated. `reject` drops a request.

**Control-plane-initiated (verified over the remote channel):**

```sh
stado_fleet enroll render-node-a --ssh operator@render-node-a.local --fleet render-burst --bootstrap
```

`enroll` first probes the machine through Stado's own remote channel and
puts the machine's REAL hostname into the entry — the registration is a
verified fact, not a declaration. An unreachable machine is refused
before any write; a failed bootstrap rolls the entry back. The channel
is anything the destination resolves over — LAN, mDNS, or an overlay
address such as a tailnet IP; Stado does not care which.

No ssh access from the control plane? Skip `--ssh` — the machine
registers with `ssh: null` and installs itself. Pass the machine's real
DNS name so the agent can resolve itself in the registry:

```sh
stado_fleet enroll render-node-c --hostname render-node-c.local --fleet render-burst
# then, on render-node-c itself:
stado bootstrap --local --target render-node-c
```

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

The machine must already be a registered target (`stado registry pull`
lists them, `stado registry host add` onboards a new one), and the fleet
must be declared first — both are checked before anything is written.

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

## Host keys in the vault

Host keys live in Skarbiec, not in home directories. The vault is the
source of truth; using a key means materializing it for one remote call
and removing it right after, and private material is never printed.

```sh
stado_fleet key add render-node-a --from ~/.ssh/id_ed25519   # import into the vault
stado_fleet key ls                                            # metadata only: id, type, fingerprint
stado_fleet key install render-node-a                         # public key -> authorized_keys on the host
stado_fleet key check render-node-a                           # verify the vault key opens the channel
stado_fleet key rm render-node-a                              # remove from the vault
```

Targets with a vault key open their channel with it (`-i` from the
materialized file); targets without one keep the OpenSSH default
resolution (agent, config, default key files).

## The central catalog

Which registration paths the fleet allows, and how machines reach the
control plane, is declared once — in the canonical registry, readable by
every machine:

```json
{
  "enrollment": {
    "allow_join": true,
    "allow_enroll": true,
    "require_verified_hostname": true
  },
  "channels": {
    "control_plane": ["loopback"],
    "notes": "anything the address resolves over: LAN, mDNS, tailnet"
  }
}
```

`stado_fleet catalog` prints it. `join`, `approve` and `enroll` consult
`enrollment` in their preflights: a path the catalog disables is refused
with the policy's own message, before anything is written. A document
without the sections is unrestricted — and `catalog` says so out loud,
so an absent policy is never mistaken for a declared one.

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

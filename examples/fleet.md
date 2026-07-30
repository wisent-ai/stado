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

One command takes a machine from "unknown to Stado" to registered, with
the fleet assignment in the same step:

```sh
stado_fleet enroll render-node-a --ssh operator@render-node-a.local --fleet render-burst
```

Add `--bootstrap` to install the agent on the machine right after
registering it (goes through `stado bootstrap`, Stado's own remote
channel). The preflight runs before any write: an already-registered name
or an undeclared fleet is refused with nothing changed.

No ssh access from the control plane? Skip `--ssh` — the machine
registers with `ssh: null` and installs itself:

```sh
stado_fleet enroll render-node-c --fleet render-burst
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

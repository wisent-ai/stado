# Changelog Unreleased (part 6)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- `stado network up|status|down|serve|connect`: Stado runs its own host network and makes it
  every host's preferred path. Each host keeps a node (its own launchd job or systemd user unit)
  that speaks the Noise IK handshake (`snow`, the construction WireGuard uses) and admits only
  keys the registry declares for a fleet host; `targets[].network` carries the public key, the
  measured endpoints and an optional relay declaration. A host's address is
  `<host>.stado.internal`: Stado's SSH client dials it through the node, OpenSSH through
  `ProxyCommand=stado network connect %h %p`. `up` makes it the `primary` path only after the
  host's own SSH server answered through the node, and files the previous primary under `lan`,
  `tailscale` or `direct`. Hosts no peer can dial attach to a relay, which carries the end-to-end
  session as ciphertext. Tailscale is an optional alternate path, never required.
- `stado fleet enroll --bootstrap` joins the machine to the Stado network right after installing
  Stado; `stado fleet approve` prints the same step after `stado bootstrap --local`.
- Stado Desktop: the console's `network` group runs `network status`, `up` and `down` on the API
  host.

### Changed

- The connection vocabulary (`stado-rs/data/fleet/connections.json`) names `direct` and says that
  Tailscale, Nebula and WireGuard are outside networks the operator runs, not ones the fleet
  configures.
- Every Stado install runs the incoming binary's `config validate` (after `config migrate`) before
  it replaces the running one: `stado product install stado`, `stado release local install` with
  or without a version, and the remote activation of a host delivery. `config validate` reports a
  host whose resolver needs `ssh.keepalive_seconds` and `ssh.keepalive_count_max`, and every
  keepalive refusal names the exact `stado config set` commands.
- The host Stado process (`com.wisent.stado`) runs the resolver role on every host. The registry
  authority answers every other host's registry read from its resolver's held generation, so an
  authority without it refused every read with "the resolver that published its state … is not
  running".

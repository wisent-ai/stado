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
- `stado network up --host HOST` joins another machine: when this machine's registry copy tells
  HOST apart from it, HOST's own installed Stado runs `network up` over its host channel (the
  node, its key and its unit are that host's), and its receipt is printed here.

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

### Fixed

- A registry authority read starts every declared path at once on Linux. A wait read its own
  command and ancestors with one `ps` each, and `ps` walks every process of the machine: on a host
  with thousands of threads that took nearly five seconds before the first path started, and the
  read was abandoned at its five-second bound. The wait-for graph now reads `/proc/<pid>` directly.
- A `.local` name the link answers late is still resolved: a resolution asks twice over three
  seconds (RFC 6762 §5.2) instead of once over one, and the answer stands for the rest of the
  command for its 120-second TTL, so the connections that follow it do not ask the link again.
- `stado network up` mints the host's key and settles its node port before it reads the registry
  authority. A host that cannot read the authority yet keeps the key, and the refusal names it.
- `stado config validate`, and so every install's configuration check, reads a resolver state
  file whose resolver no longer runs as no resolver: it says so in a `NOTE` line, which the
  install relays, and no longer asks the host for the SSH keepalive a running resolver needs.

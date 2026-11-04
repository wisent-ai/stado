# Changelog Unreleased (part 22)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **Choosing a host's path no longer starts the Tailscale client:** Stado judged every declared
  SSH path, its own network's `<host>.stado.internal` and LAN names included, by asking this
  machine's `tailscale status --json` whether the tailnet carried traffic to it. A Tailscale client
  that never answered therefore held every host command and the resolver's authority read on a
  fleet that does not route through Tailscale at all. A path is now judged by the tailnet only
  when its host is a MagicDNS name or an address Tailscale assigns (100.64.0.0/10,
  fd7a:115c:a1e0::/48), read from the destination's own text; every other path is tried as
  declared and the client is never started for it. A storage URL whose address is outside those
  ranges is likewise refused as plain HTTP without asking the client whether the address is a
  tailnet node.
- **A host its own network reaches never waits for Tailscale:** a host declaring its own-network
  primary, a LAN name and a Tailscale address as a last alternate still asked the client to judge
  that alternate. While a host declares any path that does not go through a tailnet, all its
  paths are now tried as declared and the client is not asked; only a host reached through the
  tailnet alone has its expired-key paths left out.
- **A public edge on a host the registry does not hold is refused up front:** `stado web route`,
  `stado web edge hostnames` and `stado web origin declare --publication web-edge` failed only
  while delivering, naming a missing target and not the edge declaration that named it. They now
  refuse before planning: `the public edge is declared on <target>, which the canonical registry
  does not hold, so no hostname can be published through it`, followed by the `stado web edge
  provision` and `stado web edge declare` commands that fix it.

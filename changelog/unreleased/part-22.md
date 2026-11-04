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

### Added

- **`stado registry history`: who changed the registry:** a registry write left only a new
  generation, so an entry an agent wrote read the same as one the operator declared, and nobody
  could say who made a change. Every write is now recorded beside the document by the store that
  took it, with the time, the generation it made and replaced, the account, host and agent session
  that made it, the command line and the dotted paths it changed; a write relayed to the registry
  authority is recorded there under its caller, with the authority beside it. `stado registry
  history [--path P] [--json]` lists them, and Desktop's Registry screen runs it. A part that
  cannot be read, a record that cannot be stored and a gap between records are each said, never
  left out.
- **An inference deployment no longer needs Tailscale:** `stado inference plan` refused a host
  without the Tailscale client, bound the endpoint only on its tailnet address, and the registry
  accepted no other visibility. `--visibility lan` (the default) binds the endpoint on a private
  address of the host's own interfaces and `--visibility tailscale` on its tailnet address; the
  inventory reads every interface address with `ip` instead of asking Tailscale, a host with no
  address on the chosen network is refused naming the addresses it holds and the visibility that
  fits, and the registry checks each visibility against its own address rule. Desktop's Inference
  screen gains a Plan and apply section with the same choice.
- **A host's link report reads its own network first:** the beacon's link block took its path only
  from `tailscale status --json`, so a host reached over Stado's own network with no Tailscale
  client reported `path_kind: unknown`. The path now comes from the host's Stado network node
  (`direct` or `relay`, with the endpoint the registry declares for it), and Tailscale is read only
  when the host has no node or its node could not tell, with that cause in `unread`. `source` names
  both readings, such as `pmset+stado-network`.
- **A store read refused at this host's resolver says why:** a command whose fleet store is reached
  through this host's resolver answered a refused connection with a bare loopback port and
  `infra_down`. When the resolver published why it does not serve that adapter (it holds no
  listener, it left the adapter unserved, or it is no longer running), the refusal now carries
  that reason and points at `stado resolver status`, with the same exit code.

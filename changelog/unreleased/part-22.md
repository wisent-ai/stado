# Changelog Unreleased (part 22)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **A host whose store a placement move fences keeps serving:** the worker took the fence's
  refusal of a write (`local storage writes are fenced by …`) for an error of its own, so the
  agent loop ended and the host's one Stado process exited with it at every start, taking its
  object API, resolver and forwards down for as long as the move's fenced copy lasted. A fenced
  store now ends the worker's tick as a store that does not answer: the tick says so with the
  fence's own sentence, which names its transaction and how to lift it, the next poll runs again,
  and the process goes on serving reads and its other roles.

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
- **Joining a machine prefers its own network over Tailscale:** the offline invite fragment
  advertised the machine's tailnet name whenever a Tailscale client was installed. It now picks
  a `.local` name that answers, then the default interface's IPv4 address, and reads Tailscale
  only on a machine that has neither, so a machine joins without Tailscale.
- **Every public origin says who last changed it:** `stado web origin list` and `status` carry
  `last_changed`, the newest recorded registry write that changed the declaration (account, host,
  agent session, time), or say it was never recorded; Desktop's Public origin row shows it as
  Last changed by.
- **An inference deployment holds one board, not the whole host:** a deployment named no board, its
  container saw every GPU, and an exclusive reservation set the host's free memory to zero, so a
  two-board host lost its second board to one engine. `stado inference plan` now names the board
  (`--gpu GPU-<uuid>`, or the one with the most free memory), the container is started on that
  device alone, and the agent withdraws only that board from admission; GPU jobs stay eligible on
  the others. `stado inference blockers` prints each process's board. A deployment planned before
  boards were named still holds every board.
- **`stado capabilities list [FAMILY] [--json]`:** the capability catalog had only its bare form,
  so `stado capabilities list` was read as a family named `list` and refused with `unknown
  capability "list"`. The listing now answers its own `list` verb beside `resolve`, `call`,
  `select` and `attestation`, printing exactly what the bare command prints; a family the catalog
  does not declare is refused under both forms naming every family, and nothing is written.
- **`stado cost host`: what each hour of an owned host cost:** owned hardware had one hand-written
  hourly figure for every host, and nothing measured a board's draw or use. Every host publishing
  its beacon on a period now samples each board's utilization and power draw and its processor's
  busy share once per period, keeps the last day of hours in the beacon, and `stado cost host
  TARGET` prices each hour's energy at the tariff the registry declares (`targets[].power_tariff`,
  refused without a source). An hour with no board sample is unpriced, never zero. Desktop's
  Registry screen runs it.
- **A job on owned hardware is priced from what its board drew:** `local_hourly_cost_usd`, one
  hand-written figure for every owned host, is removed. A local job records the board it ran on
  (`gpu_uuid`), and `stado cost report` prices it from that board's measured energy in each hour
  it ran, shared among the jobs on the board, at the host's USD `power_tariff`; the optimizer
  prices an owned offer at its busiest board's mean measured hourly energy. A host with no tariff,
  a tariff in another currency or no measured hour is unpriced with that cause, never free.
  `stado cost quote` answers an owned allocation by pointing at `stado cost host`.
- **`stado market earnings`: what a host earned, beside its cost:** income on Vast.ai was read only
  as a live listing, so no command said what a host had earned. `stado market earnings --provider
  vast --target HOST [--since-days N]` reads Vast's own per-machine, per-day statement of GPU,
  storage and bandwidth income, keeps each stated day in the fleet store
  (`market/earnings/vast/<host>/<day>.json`), and prints each day beside the host's measured cost
  of that day and the margin; a day Vast did not state is printed as not stated, never zero.
  Desktop's Registry screen runs it.
- **`stado inference route audit`: routes that pay outside for a model the fleet serves:** a route
  could send traffic to an outside provider for the very model a running deployment serves, and
  nothing said so. The audit lists each such route or fallback with the deployment, its host and
  the `route set` that makes the deployment the primary and keeps the outside provider as the
  fallback; it writes nothing and exits non-zero on a finding. Desktop's Inference screen runs it.
- **Peer lending keeps the boards an inference engine does not hold:** `stado market follow
  --provider stado` drained the lending worker for any inference reservation, so a host serving a
  model on one board lent nothing from its others. A reservation that names its board now leaves the
  worker running: it is started with `STADO_INFERENCE_RESERVATION` naming this host's reservation,
  and its admission gate withholds that board alone. A reservation naming no board, or one that
  cannot be read, still drains it. The Vast loop still withdraws the whole machine, because Vast
  offers no way to keep one GPU of a listed machine from renters.

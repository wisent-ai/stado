# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.69 – 0.23.70](changelog/0.23.69-0.23.70.md)
- [0.23.68 (part 1 of 10)](changelog/0.23.68-part-1.md)
- [0.23.68 (part 2 of 10)](changelog/0.23.68-part-2.md)
- [0.23.68 (part 3 of 10)](changelog/0.23.68-part-3.md)
- [0.23.68 (part 4 of 10)](changelog/0.23.68-part-4.md)
- [0.23.68 (part 5 of 10)](changelog/0.23.68-part-5.md)
- [0.23.68 (part 6 of 10)](changelog/0.23.68-part-6.md)
- [0.23.68 (part 7 of 10)](changelog/0.23.68-part-7.md)
- [0.23.68 (part 8 of 10)](changelog/0.23.68-part-8.md)
- [0.23.68 (part 9 of 10)](changelog/0.23.68-part-9.md)
- [0.23.68 (part 10 of 10)](changelog/0.23.68-part-10.md)
- [0.23.67](changelog/0.23.67.md)
- [0.23.66 (part 1 of 2)](changelog/0.23.66-part-1.md)
- [0.23.66 (part 2 of 2)](changelog/0.23.66-part-2.md)
- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

### Fixed

- **A resolver no longer holds every client for a host none of whose SSH paths answers:** each
  connection to a service placed on such a host tried every path again, so a service on a host
  that went away held each client for as long as its paths took to fail, and the caller gave up
  first. The resolver now remembers, per active host, that its last session attempt was refused
  by every path and why, and refuses a connection to that host within the refresh interval at
  once with an HTTP 502 naming that refusal (`not tried again: every SSH path of <host> refused
  <N>s ago, within this resolver's <N>s refresh interval (…)`). The first connection after the
  interval tries again; a session that opens forgets it. Real journey:
  `tests/resolver/unreached.rs`.

- **A CLI that carries a resource bundle passes readiness:** a CLI stage places a directory under
  `bin/` beside its commands (a SwiftPM `<Package>_<Target>.bundle` that `Bundle.module` reads)
  whole, with no link on PATH, but readiness judged every recorded path as an executable, so the
  bundle failed as `<bundle> is not executable` and every installation of such a CLI was rolled
  back. A placed directory is now passed over; a recorded path that is missing or unreadable is
  still refused as not executable.

- **Azure VM creation is decided by ARM's `error.code`, not by words in its messages:** a NIC or
  VM create refusal was classified by searching the message for `QuotaExceeded`,
  `OperationNotAllowed` or `SkuNotAvailable`, so a message that quoted one of them for another
  reason skipped a location and one that phrased it differently did not. A refusal ARM states is
  now kept with its HTTP status and `error.code`, from the response and from a long-running
  operation that ends Failed or Canceled, and a location is skipped only for those codes. Any other
  refusal is no longer dropped: when no location places the VM, the create fails naming each
  location's refusal. A NIC that cannot be removed after a refused VM, or after a delete, fails the
  call naming the VM, the NIC and both refusals instead of being logged and left holding the name
  the next attempt needs.

- **An Azure endpoint on an IPv6 link-local address is admitted like an IPv4 one:** a declared
  `azure.cloud.*` endpoint on `fe80::/10` was refused while `169.254.0.0/16` was admitted, though
  both are link-local; both now are.

- **One tick at a time runs over a store:** every coordinator tick now holds
  `state/coordinator/tick-lock.json` naming its process while it runs, so two ticks over one store
  no longer assign, dispatch and reap the same jobs twice. A record whose process on this host is
  gone is taken over; one still running, or held on another host, refuses the tick naming its pid,
  host and start. A resident coordinator that finds the store held logs `tick skipped: …` and
  ticks at its next interval.

### Added

- **A product's submit policy is declared through `stado product registry set --submit`:** the
  submit queue refuses every change of a product whose catalog record declares no `submit`
  policy, naming the declaration it needs, but no verb could write one, so such a product's
  changes had no queue path. `--submit` takes the whole declaration as one JSON object
  (`criticality`, `max_changed_lines`, `low_risk_max_changed_lines`, `review_paths`) and replaces
  the record's `submit`; an unknown field is refused by name, and the catalog validation every
  write runs refuses a standard product without a low-risk limit, a critical one with it, a zero
  limit, an unknown criticality and a review path outside the repository. Stado Desktop's "Update
  a catalog product" form carries the same field.

- **The Azure cloud is declared, not built in:** the Resource Manager, the OAuth authority, the
  managed-identity endpoint and the Retail Prices API were constants of the public cloud,
  scattered through VM dispatch, the credit gate, sign-in, role repair, billing, the SKU catalog
  and quota replies. They are now the compute bindings `azure.cloud.resource_manager`,
  `azure.cloud.authority`, `azure.cloud.identity_endpoint` and `azure.cloud.retail_prices`
  (`AZURE_RESOURCE_MANAGER`, `AZURE_AUTHORITY`, `AZURE_IDENTITY_ENDPOINT`, `AZURE_RETAIL_PRICES`),
  each the public cloud's when undeclared, so a deployment can rent from a sovereign cloud and a
  listener on a loopback address can stand in for Azure. A declared endpoint that is neither
  HTTPS nor on a loopback or link-local address refuses every Azure call naming the binding, and
  `stado config show` names the endpoints in force. The ARM client no longer has an
  unauthenticated mode: every request carries the token.

- **`stado scheduler tick`: one coordinator tick, now:** the tick that settles cancellations,
  fires due schedules, reaps leases, assigns and dispatches ran only inside a coordinator's loop,
  so nothing placed a queued job or fired a schedule before the next interval, and nothing could
  observe one tick end to end. `stado scheduler tick [--provider P]... [--billing] [--json]` runs
  the same tick once in this process, holding the store's tick lock, and answers how many machines
  it started and which arms ran; a provider that is not a compute provider, is fenced or not
  enabled, or has no tick arm, and a store another tick holds, are refused before the tick.

# Changelog Unreleased (part 13)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

- **`stado product remove PRODUCT...` uninstalls whole products in one command:** removing a
  product took one `--surface` per call, so uninstalling Tama, Stado and Oko meant knowing which
  of `cli`, `desktop` and `service` each had installed and running `remove` once for each.
  Without `--surface`, `remove` now takes one or more products, reads every receipt under
  `~/.stado/products/<product>/` and removes each surface recorded as installed (service, then
  desktop, then CLI) through the same removal that `--surface` runs, with the same ownership
  checks and backups; the product whose installation placed the running `stado` goes last. A
  product with no installed surface is refused with `<product> has no installed surface on this
  machine: no receipt under <home>/.stado/products/<product> records one`, a product named twice
  is refused before anything is removed, a failed removal names the products already removed,
  and a missing receipt for a named surface now names its path. Stado Desktop's Products screen
  has the same operation as *Uninstall products*.
- **`stado product create` refuses to create what already exists:** a creation request was
  checked only for a catalog product with the same id, so an existing repository proposed under
  a new name went through as a "new" product. Before the first repository is provisioned, the
  request is now held against every repository the catalog claims and every repository each
  requested owner already holds on GitHub (the list `stado product catalog --unclaimed` reads),
  and refused, naming the repository and the product that claims it, when a requested
  repository exists or the product id is the name of an existing repository. A repository the
  same request provisioned on an earlier run is its own, so `--resume` still works.
- **Stado's roadmap names OpenRouter as a way to sell tokens:** the catalog's roadmap for Stado
  carries *Become an inference provider on OpenRouter*, and `/docs/earning` says what it needs.
- **`stado web origin declare` defaults to our own publication:** with no `--publication`, a
  public origin was declared `tailscale-funnel`, so the default way to make a fleet name public
  depended on an outside network provider's control plane. The default is now `web-edge`, which
  publishes through the `stado web` declaration that owns the hostname on the fleet's own
  declared edge. `tailscale-funnel` is still accepted when it is named.
- **The database client's cancellation reaches every engine's connect:** `connect_cancellable`
  passed the caller's cancellation to the credential resolver and to the Postgres pool, but a
  MySQL or SQLite pool connect still waited without it. Every engine's pool now opens through
  one wait that honours the cancellation and refuses with the step and `czekanie odwolane`.
- **`stado doctor` names why the release manifest could not be fetched:** the release channel
  check reported reqwest's own `error sending request for url (...)`, which hides why TLS or the
  connection failed. It now names the stage (`connecting or the TLS handshake failed`, `the
  request timed out`, `the request failed after it was sent`) and the whole chain of causes,
  such as an unknown certificate issuer, an expired or mismatched certificate, or a refused
  connection.

- **`stado sandbox fleet` places and finds sandboxes across the fleet without `--host`:** every
  fleet sandbox operation named its host, so work went where the caller pinned it, not where
  there was room. `--host` is now optional on every operation but `inventory`. `create` reads
  every registered Linux host at once with one read-only program (gVisor `runsc`, cgroup v2,
  `--nvproxy`, `nproc`, `MemAvailable`, `MemTotal`, NVIDIA GPUs, and the limits its running
  fleet sandboxes hold) and puts the sandbox on a host with the declared cores, RAM and GPUs
  free, the one with the most free RAM first; every named operation runs on the host that keeps
  the sandbox; `list` lists every host's sandboxes with their host. A name is kept unique across
  the fleet, a host that cannot be read is named and does not stop the others, and the receipt
  carries `placement` with the verdict on each host; no host with room is refused naming every
  host and why. Stado Desktop's Fleet sandboxes take the host as optional.
- **`stado product install` installs a Cargo surface again:** planning prerequisites read every
  recipe's `manifest` as a release manifest, so a Cargo installation's `Cargo.toml` was parsed
  as JSON and each install or `--plan` of one was refused with `parsing release manifest
  most-rs/Cargo.toml … expected value at line 1 column 2` (Most's service, Skrzynka, Skryba and
  every other Cargo recipe). Only a Stado release's manifest is read for verb contracts now; a
  recipe of another kind declares no verbs.
- **A unit holding an option a later build removed no longer blocks every upgrade:** a newer
  Stado read each running `stado serve` unit's argument vector with its own parser, so a unit
  rendered by an older build with an option since removed (`--disk-cleanup`) was refused as
  `cannot be read by this build's serve: unexpected argument`, `product update stado` could not
  retire it (`the units stado replaces could not be read, so none was retired`) and the host
  could never be upgraded. The serve line is now read from the options this build declares;
  an option it does not declare is set aside with its value as opaque text, read as no role, and
  `host` role reports name it on stderr. A line that still does not parse is refused as before.

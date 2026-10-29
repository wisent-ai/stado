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

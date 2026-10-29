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

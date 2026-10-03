# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.15](changelog/0.23.12-0.23.15.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The units a product's one process replaced are found on each host from what they run; the catalog lists none:** `retired_units` and `role_units` are gone from `catalog/products.yml`, and the catalog validator refuses either key. A unit on a host that runs a catalog product's program under any label but that product's one unit is its predecessor: a program in the product's own service tree, the exact program the catalog declares, or a file named after the product wherever it was installed. Any product but Stado replaces such a unit whole; for Stado the unit's work is read from its own command line by Stado's own command definitions (`serve` options, `dashboard`, `agent`, `coordinator`, `resolver serve`, `release agent`, `product sync`, the beacon commands, `stado-watchdog`, `stado-fix`) or matched to the live process's runner root, edge program or forward destination, and it is retired only once that process is proven to run every such role; one with no role is reported `kept` with the reason. `stado service ensure`, the autonomy reconciler, the API takeover at start, the object-API recovery, `service retire`, the registry doctor, the release-unit revisit block and the deployer configuration all decide from this one rule, so an old unit found on a host needs no catalog edit and no release. The reconciler now also retires an undeclared fleet unit no product owns when nothing runs it and no launchd domain holds it, and reports each live one as `undeclared_live` with its program instead of leaving it unnamed. `service list --undeclared` and every reader of a host's units read Linux systemd unit files too, where a Linux host used to answer that it held no unit. `stado service serve-roles` also prints `STADO_SERVE_ROLE_PATHS`.

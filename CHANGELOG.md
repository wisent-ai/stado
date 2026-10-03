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

- **Replacing an installed program checks the commands installed products were seen running of it:** every `stado` invocation records, once per caller and command, which executable started it and which command it ran (`~/.stado/callers/`). `stado product install|update` and `stado release install-local` read the records whose callee is the program being replaced, keep those whose caller is another product's installed path and has not changed since, and ask the incoming program for each recorded command with `--help`; one it does not answer refuses the installation with the product, its source revision, the caller and the command, so a Stado that dropped `secrets get` no longer replaces the binary an installed Oko still calls it through. No list of callers or commands is kept by hand (4aade303).

- **`stado web vercel` is gone; every web product builds with `stado web build` and is hosted by Stado:** the last of the six products that built and delivered through Vercel (wisent-trade) now runs `stado web quality` and `stado web build` on its `web` platform, so `stado web vercel build`, `stado web vercel deploy` and the enrolment refusal that named them are removed. A manifest that still names the command fails at the step that runs it with clap's unknown-subcommand error (357b0c1f).

- **The units a product's one process replaced are found on each host from what they run; the catalog lists none:** `retired_units` and `role_units` are gone from `catalog/products.yml`, and the catalog validator refuses either key. A unit on a host that runs a catalog product's program under any label but that product's one unit is its predecessor: a program in the product's own service tree, the exact program the catalog declares, or a file named after the product wherever it was installed. Any product but Stado replaces such a unit whole; for Stado the unit's work is read from its own command line by Stado's own command definitions (`serve` options, `dashboard`, `agent`, `coordinator`, `resolver serve`, `release agent`, `product sync`, the beacon commands, `stado-watchdog`, `stado-fix`) or matched to the live process's runner root, edge program or forward destination, and it is retired only once that process is proven to run every such role; one with no role is reported `kept` with the reason. `stado service ensure`, the autonomy reconciler, the API takeover at start, the object-API recovery, `service retire`, the registry doctor, the release-unit revisit block and the deployer configuration all decide from this one rule, so an old unit found on a host needs no catalog edit and no release. The reconciler now also retires an undeclared fleet unit no product owns when nothing runs it and no launchd domain holds it, and reports each live one as `undeclared_live` with its program instead of leaving it unnamed. `service list --undeclared` and every reader of a host's units read Linux systemd unit files too, where a Linux host used to answer that it held no unit. `stado service serve-roles` also prints `STADO_SERVE_ROLE_PATHS`.

- **The managed-product declaration names only Stado's own unit:** the old labels listed under `stado` in `stado-rs/data/catalog/products.json` are gone. After a new Stado binary is installed, the host release restarts its own unit and every registry unit on the host that still runs the Stado program under another label, found from that program.

- **Signing and installation secrets are read by role, never by item name:** `stado product install` and `stado product signing` no longer name the vault item `desktop-signing-apple-development`; they read the certificate and key of the item playing `stado:role:macos-development-signing`, and `WISENT_CODESIGN_ROLE` replaces the removed `WISENT_CODESIGN_CREDENTIAL_ITEM`. A source installation reads each `role#field` of a release manifest's `secret_env` by role, as the release path already did. The general read is `stado credentials get --role ROLE [--field F]`; no item in the role, or several, is refused with the role and the command that stores it. Before this release signs anything, the vault owner must run a Skarbiec that registers the `stado:role:` namespace and the signing item must carry the role: `stado credentials item retag --host <vault owner> <item> --tags stado:role:macos-development-signing`.

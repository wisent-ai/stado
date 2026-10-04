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

- **An `after_install` step can name the host it installs on:** `{host}`, anywhere in an argument, is replaced with the registry name of the host the installation placed the product on, so a service's install can declare per-host state under its own identity, such as a maintenance schedule pinned to that host (`--pinned-host {host} --id <product>-maintain-{host}`). A step that names `{host}` in an installation with no host fails naming the placeholder (1cff0c87).

- **The host-health bearer is read by role, never by item id:** the beacon that publishes and the dashboard that verifies `PUT /api/host-health` both read the `token` of the item tagged `stado:role:host-health-api`, and the object-verifier validation finds the host-health item the same way (two items in the role are refused). No item id `host-health-api` is written in Stado any more, so the vault owner may name or replace that item freely. A beacon whose grant sees no item in the role is refused with the `stado credentials item retag … --tags stado:role:host-health-api` command that tags it. Before this release publishes beacons, the vault owner tags the item holding the host-health bearer with that role (62a125ae).

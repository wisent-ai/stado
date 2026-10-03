# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.1](changelog/0.22.18-0.23.1.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado service retire` and `remove` withdraw every service-directory route on the host whose `managed_service` names the withdrawn service, by service name or by unit id, in the same registry write as its managed record. Before, a route with its own name (such as `skarbiec`) that named a launchd service by its service name survived the withdrawal, and the registry refused the write with `managed_service: is not declared on the active host`. `tests/service/removal.mjs` qualifies a dedicated launchd test unit carrying such a route and checks the record, routes, unit file and launchd job afterwards.

# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.47](changelog/0.23.42-0.23.47.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado resolver api-reassign --target HOST [--json]` gives a host's resolution API a port the host hands out:** a resolver's `api_bind` was the last loopback port still written into the registry by hand (8d8c8cac); adapters already move with `service directory consumer-add --reassign`. The command asks HOST for a free loopback port (`stado host free-port-local` run there through its Stado channel), records it as `targets.<host>.service_resolver.api_bind` under the registry generation it read, and prints `HOST: resolver API from <old> to <new> (generation N)` or, with `--json`, `{target, previous, api_bind, generation}`. The host's resolver rebinds its API when it reads the registry. Refusals: `<host> declares no service_resolver, so it has no resolution API to move` (not_found), a host whose Stado cannot hand out a port (`the host could not hand out a free port …; install the current Stado there`), and a registry that moved since the read.

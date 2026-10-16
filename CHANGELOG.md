# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

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

- **`stado resolver snapshot` gives up on a resolver that accepts and answers nothing at that resolver's refresh interval:** the authority's snapshot command asked its own resolver's API with no measure, and the reader that started it over SSH gave up at the refresh interval and closed its channel, so each abandoned read left a snapshot process and its connection on the authority; hundreds of them held the authority's connections and file descriptors. The command now stops at the refresh interval this host declares and refuses with `the resolver answering at <bind> accepted the request and answered nothing: `stado waits --json` lists what it waits on, and `stado service logs com.wisent.stado` says what its unit is doing`. A resolver API that answers an error body that is not JSON is refused with that body and the parse error instead of a bare status.
- **A directory read that cannot reach its authority no longer waits on that authority's store to record the refusal:** the `authority_unreachable` refusal was written to the fleet store and awaited, and that store is served from the authority host the read had just failed to reach, so each failing read (every command that reads the registry) waited on that host a second time before its caller saw the refusal and added one more write to the queue in front of it. The record is now written detached, as the resolver's `directory_cache_stale` refusal already was: a one-shot command returns its refusal at once and loses its own copy if it exits first, and the resolver on the same machine, which reads the same authority, records the refusal from its long-lived process. `report_refusal`, the awaited publisher, is gone.

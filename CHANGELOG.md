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

- **The resolver says whether a service answers, not only what it still holds:** a request left `~/.stado/resolver-waiting.json` the moment its client closed, so a service every client gave up on before it answered showed only young waits and read as healthy while Brama answered none of the judged hooks' requests. Every relayed request that ends is now recorded per service in `~/.stado/resolver-answers.json`: `last_answered_at`, and while the service has answered none since, `first_unanswered_at` and `last_unanswered_at`; the first answer clears both. Reading a request line no longer drops a decoding error: bytes that are not text are shown as the replacement character.
- **A `linux-account` scratch lease is entered instead of deleted again on Linux:** the program that trusts a new lease account with the keys that reach its host called `/usr/sbin/chown`, a path macOS has and Ubuntu does not, so `stado scratch create` on a Linux host removed the account it had just created and refused with `chown: not found`. The program now writes `authorized_keys` with `/usr/bin/install -m 600 -o <account>`, the tool it already uses for the `.ssh` directory and which every declared platform carries.
- **`stado service serving <name> --wait` reads the declared port through the unit the host declares for `<name>`:** asked by product name (as `stado product install stado` asks after replacing the resident unit), it found the host's unit `com.wisent.stado` but looked the directory up by the product name alone, so a directory that keys the endpoint under another service name (`managed_service: com.wisent.stado`) answered "declares no endpoint" and the install rolled back a candidate that was serving. When the name is not a directory key, the declared unit's label now selects the endpoint, and the refusal names both the name and the unit it tried.
- **`stado resolver snapshot` gives up on a resolver that accepts and answers nothing at that resolver's refresh interval:** the authority's snapshot command asked its own resolver's API with no measure, and the reader that started it over SSH gave up at the refresh interval and closed its channel, so each abandoned read left a snapshot process and its connection on the authority; hundreds of them held the authority's connections and file descriptors. The command now stops at the refresh interval this host declares and refuses with `the resolver answering at <bind> accepted the request and answered nothing: `stado waits --json` lists what it waits on, and `stado service logs com.wisent.stado` says what its unit is doing`. A resolver API that answers an error body that is not JSON is refused with that body and the parse error instead of a bare status.
- **A directory read that cannot reach its authority no longer waits on that authority's store to record the refusal:** the `authority_unreachable` refusal was written to the fleet store and awaited, and that store is served from the authority host the read had just failed to reach, so each failing read (every command that reads the registry) waited on that host a second time before its caller saw the refusal and added one more write to the queue in front of it. The record is now written detached, as the resolver's `directory_cache_stale` refusal already was: a one-shot command returns its refusal at once and loses its own copy if it exits first, and the resolver on the same machine, which reads the same authority, records the refusal from its long-lived process. `report_refusal`, the awaited publisher, is gone.
- **`stado product dependencies` no longer reads a product's documentation or examples as calls:** a product named in a top-level `docs/` page (any case) or in a file under an `examples/` directory at any depth was reported as a dependency the product had not declared, so a page describing another product, or an example script a person copies, asked for a catalog alternative the running product never needed. Both are now left out with the tests, prose, forge automation, lockfiles and release tooling the audit already skips; line numbers are still counted from one.

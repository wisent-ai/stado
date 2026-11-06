# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.68 (part 1 of 10)](changelog/0.23.68-part-1.md)
- [0.23.68 (part 2 of 10)](changelog/0.23.68-part-2.md)
- [0.23.68 (part 3 of 10)](changelog/0.23.68-part-3.md)
- [0.23.68 (part 4 of 10)](changelog/0.23.68-part-4.md)
- [0.23.68 (part 5 of 10)](changelog/0.23.68-part-5.md)
- [0.23.68 (part 6 of 10)](changelog/0.23.68-part-6.md)
- [0.23.68 (part 7 of 10)](changelog/0.23.68-part-7.md)
- [0.23.68 (part 8 of 10)](changelog/0.23.68-part-8.md)
- [0.23.68 (part 9 of 10)](changelog/0.23.68-part-9.md)
- [0.23.68 (part 10 of 10)](changelog/0.23.68-part-10.md)
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

### Added

- **A database backup is declared as a routine of this host:** nothing on the operator's machine backed a declared database up on its own, so a database whose host stopped answering could only be restored from whatever backup someone had last taken by hand, and `stado registry doctor` could name it stale but nothing kept it fresh. `stado database backup [NAME] [--keep N] --schedule CRON` declares, in this host's own store, a routine that runs `stado database backup [NAME] [--keep N] --json` on CRON (UTC), under an identity derived from NAME: the same declaration again changes nothing, another expression or retention brings the one routine to it with its run history kept, an undeclared NAME is refused (`database "NAME" is not declared`) and so is an invalid expression (`invalid cron expression`). The routine runs whether or not the fleet store answers. Journey `tests/database/backup-migrate.mjs` declares it, declares it again, moves it and reads it back from the host store.

### Fixed

- **A public edge on a host the registry does not hold no longer refuses every Stado install:** `stado config validate` refused such an edge, and every install asks the incoming binary that question before it replaces the running one, so a host whose web edge named an unregistered host received no release at all, and with it none of the repairs it carried, though the incoming binary runs with that declaration exactly as the installed one does. Every install (`stado product install stado`, `stado release local install`, `stado host upgrade`, the remote activation step) now asks `stado config validate --install`, which refuses only what the incoming binary cannot run with and prints the edge sentence as `NOTE <sentence>; an install runs with this declaration, so it does not refuse one`, relayed by the install. Plain `config validate`, `web edge status`, `web route`, `web edge hostnames` and `web origin declare --publication web-edge` still refuse the edge naming the commands that fix it. A release older than the flag refuses `--install` itself and is asked the plain `config validate` it answered before; `--install` on another `config` verb is refused (`--install judges a document as an install does; it belongs to config validate, not config <sub>`, exit 2). Test `tests/web/edge_target.rs`: the install verdict accepts the edge with its note, still refuses a document whose `providers` is no list, and `config show --install` is refused.

- **A product sync that finds another sync holding its lock answers busy instead of failing:** `stado product sync` meant to answer `{"state":"busy","holder":...}` and exit 0 when another sync held `~/.stado/products/sync.lock`, but it recognised a held lock only as an `io::Error` of kind `WouldBlock`, which the lock stopped returning once it began naming its holder; every overlap of the desktop and cli syncs (the host routine fires both every quarter hour, and a desktop build outlasts one) then failed with `another writer owns ...sync.lock` classified `[unknown]`, and the routine recorded a failed job. A held lock is now the typed `Busy` refusal (path and holder clause, the same sentence for every other caller), and sync answers it with `{"state":"busy","lock":<path>,"holder":"held by pid <pid> (running) since <time>: <command>"}` and exit 0. Journey `tests/product/held/sync_busy.mjs`.

- **`stado database migrate` keeps the item it replaces in a vault that refuses duplicates:** the move kept the database's item as it stood under `<item>-previous` before placing anything, and Skarbiec refuses a second item holding exactly another's payload (`<item> already holds exactly this payload; an exact duplicate is refused`), which the copy always does when it is written, so every move stopped there with `[config]` and a database whose provider no longer answers could not be moved at all. A kept copy (`<item>-previous`, and `<item>-fleet-pending` after a failed placement or restore) now carries `context.kept_as {role, of, at}`, naming what it is, which item it stands for and when it was kept, so it is not the same payload and a reader of the vault sees why it is there; the item in use never carries it.

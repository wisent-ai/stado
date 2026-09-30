# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.17](changelog/0.22.17.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado host ping` answers while the object store is down (ffd7f928):** the command opened the beacon store before anything else and exited `infra_down` when it could not, so during an object-API outage nobody could ask whether a host answered ssh. The store's failure is now the beacon half's answer (`unreadable: the beacon store did not open: …`), and the ssh half is still probed and reported.

- **`service directory connect` reads the registry once (a29cca4c):** the verb read the registry for the directory, then twice more to learn which host it runs on and where the service is placed. On 2026-09-29, with the object API refusing, each read spent its retries before falling back to the last-good copy, so one connect outlasted the 30-second limit of every agent hook that asks for Brama's address and every hooked tool call of every session was refused. All three answers now come from the one document it read.

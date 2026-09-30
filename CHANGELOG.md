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

- **A refused channel no longer queues every adapter connection behind new SSH handshakes (d73f4df5):** while the remote object API refused channels, every refusal dropped the healthy SSH session, so the next connection opened a new session, and every open ran under the one lock all services and consumers share; connections queued behind each other's handshakes for 2 to 11 minutes and every agent hook that reached Brama through Stado hung with them. A session is now opened outside the shared lock and dropped only when it is closed; a channel the service refuses is reported to the caller at once and the session stays.

- **The resolver adapter says which channel opens are still waiting (d73f4df5):** on 2026-09-30 connections through the local object-API adapter waited 2 to 11 minutes while its log showed only the opens that failed at once. Every channel open to a remote host now logs when it is sent, how many opens are waiting for an answer, and when it is answered (open or refused) with its elapsed milliseconds, so a connection held without end names the host and the time it has waited.

- **`stado host ping` answers while the object store is down (ffd7f928):** the command opened the beacon store before anything else and exited `infra_down` when it could not, so during an object-API outage nobody could ask whether a host answered ssh. The store's failure is now the beacon half's answer (`unreadable: the beacon store did not open: …`), and the ssh half is still probed and reported.

- **`service directory connect` reads the registry once (a29cca4c):** the verb read the registry for the directory, then twice more to learn which host it runs on and where the service is placed. On 2026-09-29, with the object API refusing, each read spent its retries before falling back to the last-good copy, so one connect outlasted the 30-second limit of every agent hook that asks for Brama's address and every hooked tool call of every session was refused. All three answers now come from the one document it read.

# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.32](changelog/0.23.12-0.23.32.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A host is live while its own promise holds, not inside a window someone picked (fab304dc):** every capacity broadcast now carries `next_by`, the time by which its worker will publish again (its `--poll-seconds` plus how long its last write took), and every reader (`host gates`, the scheduler's live set, makespan placement, the live GPU ladder, the build fallback, `fleet claim`) counts a row only until that time. The 180-second `CAPACITY_STALE_SECONDS`, `LIVE_CAPACITY_TTL_S`, makespan `HEARTBEAT_TTL_S`, the one-hour capacity GC age, the 200-row GC cap per tick and the 30-second live-GPU cache are gone: an overdue cloud consumer's row is deleted where it is found, a local host keeps its last row as evidence. While the tick works, the heartbeat republishes as long as the call the tick is in has run no longer than that call has ever taken to return in this process, and stops (`heartbeat: the tick has been in <phase> for <n>s (the longest it has taken to return in this process is <m>s …)`) once it runs past that or in a call that never returned. Host beacons published by `stado serve --health-interval-seconds` carry `next_by` the same way, and `registry doctor` reports `stale-beacon` once it passes or when a beacon states none; a capability measurement holds until the host is measured again. Rows and beacons from older releases are held to the `stale_after_seconds` they stated.
- **`stado host ping` grades a beacon by the promise its host made (fab304dc):** a beacon was `stale` after the 15 minutes the job-lease window happened to be; it is now `stale` once the `next_by` its publisher wrote has passed (a beacon from an older Stado is held to the `stale_after_seconds` it stated), and a beacon that states neither is `stale` with that reason in `beacon.error`. `--json` carries `beacon.next_by`; the top-level `stale_after_seconds` is gone.

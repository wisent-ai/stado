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
- **The registry and the model policy are read when they are asked for, not held for a chosen number of seconds (fab304dc):** `GCS_REGISTRY_TTL_SEC` (30 s) and `MODEL_POLICY_TTL_S` (300 s) are gone. Every registry read goes to the authority, so a host moved by the operator is never answered from a copy made before the move (the separate `fetch_registry_authoritative` and the cache-clearing call after a registry import go with it); the coordinator and agent ticks read `config/model_overrides.json` on each pass, so a policy change is in force from the next one.
- **A reservation is renewed on its host's own period, and its lifetime is the holder's promise (fab304dc):** the 60-second renewal, the 180-second lifetime (three renewals) and the one-hour keep of expired rows are gone. `stado capacity hold` and every placed workload renew at once and then on the period the host's capacity row states (re-read each round), each renewal promising that period plus how long the holder's last round took; the agent deletes a row as soon as its holder's promise has passed. A host that has never published a row stating its period is refused (`… publishes no capacity row that states its period …, so nothing would read a reservation there`), recorded for `stado fleet needs` as `no_eligible_target`.
- **Measurements and run times reach scheduling on the next tick, not after a ten-minute cache (fab304dc):** the observed-VRAM map was held for `OBSERVED_MAP_TTL_S` (600 s) and the makespan run-time history for `HISTORY_TTL_S` (600 s), so a model's first measured peak waited up to ten minutes before queued jobs of that model were sized from it. Both now read the `completed/` (and for sizing `failed/`) listing and the live GPUs every pass and download the records again only when that listing or the live GPUs changed; the sizing pass and dispatch bucketing read the map once per pass. Journey `tests/scheduler/sizing.rs` (target `scheduler-sizing-measurement`): the tick after a measurement lands sizes the queued job from it.
- **GCP exhaustion marks hold until an event disproves them (fab304dc):** a zone stockout was skipped for 300 s, a regional quota refusal for 60 s, and both blobs were cached in process for 10 s. A mark now holds until a VM is created in that zone (clearing the zone and its region's quota mark for that accelerator) or deleted in that region (clearing the region's quota marks); each create call tries every unmarked zone and then the one zone whose mark is oldest, logging every zone it skips with the time it was marked, so every exhausted zone is retried in turn and no time decides when. A failed mark write is logged instead of being dropped.
- **The NVIDIA driver probe answers for the tick that asks (fab304dc):** `CUDA_PROBE_CACHE_S` (30 s) is gone; an idle NVIDIA host runs `nvidia-smi` once per tick, and `gpu_driver_detail` carries the probe's whole output instead of its last 300 characters.

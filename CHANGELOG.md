# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.61](changelog/0.23.42-0.23.61.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado schedule show` says why a schedule has not fired:** three Oko routines due every minute stayed at their first `next_due_at` for ten minutes, and `schedule show` printed the stale time with nothing beside it; the coordinator on charless-mac-mini did tick, but only once in that window. Every coordinator tick now records its schedule sweep — when, on which host, how many it fired, or the store error that stopped it — at `system/schedule-sweep.json`, and `schedule show` adds it as `last_schedule_sweep`. A schedule still waiting past its next run gets `overdue`: an occurrence reserved and not yet enqueued (with its owner), no sweep ever recorded in the store the command read (no coordinator ticks against it), the last sweep older than the due time (that coordinator has not ticked since; its tick log on the named host says what holds it), or a failed sweep with its error.

- **`stado inference route show --probe-bearer-role ROLE#FIELD` asks every route for an answer:** route show compared the registry's declaration with the gateway's served table, so a route reported as agreeing while its destination refused every request — the aliases routed to an OpenAI account out of quota looked healthy. With the flag, Brama's own `brama probe` runs on the gateway host through the host channel, one short request per declared alias with the client bearer the caller names, and each row carries `answers` and the probe's answer; a destination that does not answer is listed under `not_answering` and the command exits non-zero, as for a diverged route. Each alias spends one provider request, so the probe runs only when asked. Stado Desktop's Inference screen has the same Probe routes action, with the bearer field and an `answers` or `not answering` badge per alias carrying Brama's refusal.

- **`stado inference route set` and `remove` survive another registry writer:** their condition is the alias's current destination (`--expected`), yet a registry publication by anyone else between their read and their swap refused them with exit 75 although nothing they were conditional on had changed; moving `model-review` right after `decision-model` failed that way (2e7c67fa). A lost generation race now reads the registry again, checks `--expected` against it, stages the gateway table again and swaps; only an alias whose destination changed is refused, with `route '<alias>' is '<current>', expected '<expected>'`.

- **Stado buys compute on Vast.ai, not only sells it:** `stado market --provider vast` could list this fleet's idle GPU on Vast.ai, but nothing rented a Vast.ai machine; `stado capabilities` said "renter provisioning is not implemented". Provider `vast-rental` is now a GPU cloud vendor behind the shared lifecycle: naming it in `providers` lets the coordinator search Vast.ai's offers for the accelerator's GPU name (`RTX 4090`, `H100 SXM`, …), rent the cheapest rentable, verified, on-demand single-GPU offer with room for the boot disk, run the agent as the container's entrypoint (`STADO_WORKER_NAME` is the instance label), read and list instances (keyset-paged), and destroy them; `stado instances list --provider vast-rental` and `stado doctor` read them. No matching offer is capacity, so the dispatcher moves to the next tier. The image is the required setting `vast-rental.image` (`VAST_RENTAL_IMAGE`); the API key is the Skarbiec item tagged `stado:role:cloud-vast-rental`, field `api_key`, a key of its own apart from the host's selling key. Provider `vast` keeps meaning this fleet's own Vast.ai host. The Compute providers screen in Stado Desktop names it; `tests/providers/gpu_cloud.mjs` covers its configuration refusals.

- **A release delivery reaches a host whose janitor never stops:** a host that cannot get under its disk watermark runs janitor passes back to back, each holding the workload lock for its whole length (about twelve minutes on lukasz-macbook), and the signed Stado release delivery — the one job admitted under disk pressure — found the lock taken at every claim. Stado 0.23.61 stayed `delivering` with that delivery `queued … not yet claimed`, and Oko refused to close any defect it carries (a7c056fd). That delivery now starts beside the pass, logged as `<job>: a janitor pass holds the workload lock; the signed Stado release delivery starts beside it`; every other job still waits for the lock. No cleaner takes what a running delivery uses: its work tree belongs to a job the queue does not report terminal, and `delivered_releases` keeps each product's newest version.

- **`stado release catalog pin-input --swiftpm` publishes a Swift package's resolution:** every SwiftPM desktop release unpacks a `swiftpm-cache` input into its source and builds offline, and every one pinned an archive made by hand in August that the fleet store no longer holds; `stado quality check` refused most-desktop, oko-desktop and brama-desktop with `… answers is absent`, and no verb could make another (b6996b35). `--swiftpm` runs `swift package resolve --disable-automatic-resolution` into a scratch of its own, packs the resulting `.build/` (checkouts, repositories, artifacts, workspace state) in name order with cleared owners and times, stores it create-only under `sources/<product>/dependencies/<name>/sha256/<digest>/source.tar.gz` and pins it with mount `<name>.tar.gz`, `extract: false`. A `Package.swift` or `Package.resolved` that differs from the revision is refused. Stado Desktop's **Publish an immutable build input** has the same toggle. Used for most-desktop: 1,611,520,610 bytes, sha256 `4e3bbaca…`, and its quality check passes.

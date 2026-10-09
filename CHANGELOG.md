# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

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

- **`stado release status` and `stado release resume` say what they are waiting on:** status read the registry, every target's release state and every recorded release run, and a resumed run queued, signed, published and delivered, each without a word, so half an hour of either could not be told from a stall. Every read of status (`[release status] read stado on <host> (stado://…): started`, `read every recorded release run`) and every step of a release run (`[release run] place the deliveries`, `read build <id>`, `queue what the build owes`, `sign and publish <platform>`, `deliver to every host`, `promote the version`, `reconcile the released service`) now writes its start and `took <n>s` to stderr; stdout and `--json` are unchanged.

- **`stado release changes submit|list` say which store request they wait on:** after its quality check, a handoff opened the job store, wrote its ticket and read the builds covering it without a word, and `changes list` printed nothing until it had everything, so a store that did not answer looked like a command that hung. Each store request now writes `[release changes submit] write ticket runs/release-changes/<id>.json on the stado store <address>: started` and `…: took <n>s` on stderr when it ends, error or not; stdout, and with it `--json`, is unchanged.

- **Stado calls Skarbiec's groups:** `stado host secrets vault push|pull` runs `skarbiec mirror push|pull`, and the agent's grant gate reads `skarbiec bond status`, instead of `sync-push`, `sync-pull` and `sync-status`, which Skarbiec withdrew. A host whose installed Skarbiec predates the groups answers these with Skarbiec's unknown-command refusal until its Skarbiec is delivered.

- **`stado release submit --commit` takes the abbreviated id `git log --oneline` prints:** it refused anything but 40 hexadecimal characters (`--commit must be 40 lowercase hexadecimal characters`), so the short id an operator or agent copies from `git log` sent them back to look up the full one. Git now resolves the id in the `--source` checkout: a full id or an abbreviation of exactly one commit becomes the full commit id; an abbreviation that matches nothing or several objects, or names something that is not a commit, is refused as `--commit <id> names no single commit in <checkout>: <Git's reason>`, and an id that is not lowercase hexadecimal is refused as before.

- **A new build sheds the run it keeps:** a build run keeps the previous attempt to measure the next one's free space, and a run an older Stado left whole kept its source export and build output until two more builds replaced it. The previous attempt now keeps only its files and the size it recorded; the free-space check reads that size.

- **Azure and Apple signing credentials are read by role:** the billing collector's Azure section reads the service principal of the item tagged `stado:role:azure-billing`, the Azure token chain reads `cloud-azure` like every other cloud provider, and native signing reads the certificate of the `macos-development-signing` role. `WC_AZURE_BILLING_SECRET` and `WC_AZURE_SECRET` are gone: the billing setting named an item id that was then looked up as a role, so a configured principal reported `no_credentials`. Tag the billing principal with `stado credentials item retag --host <vault owner> <item> --tags stado:role:azure-billing`.

- **A host no longer refuses every job after its disk drops below the threshold:** a janitor that found the volume full while jobs held their shared locks asked them for its turn, and only a pass that took the exclusive lock withdrew that request. Once a finished job freed enough space, every pass ended as `healthy_noop` without the lock, so the request stood for as long as the agent lived and `stado host gates` reported `cleanup_in_progress` with the volume below 80%, holding release builds in the queue. A pass below the threshold now withdraws the turn.

- **A release continues once its build job wrote a passed receipt:** `stado release resume` read the queue before the job's receipt, and a running job the reaper had put back in the queue (`worker lease expired`) while its worker went on to finish kept answering `release job … is still queued … no host has claimed it`, while `stado build status` called the same platform passed. The receipt, which only the worker that ran the job writes and every publish verifies, is now read before the queue.

- **A release job requeued while its worker kept building is not built twice:** when the reaper requeued a running release job (`worker lease expired`) and its first worker went on to write a passed receipt and archive, the job stayed queued for a second build. Each reaper pass now completes such a job from the queue once its receipt and archive verify against the job's immutable request and were written after the job was created, the same evidence a lapsed running job is completed from (`<job>: requeued on an expired lease, completed from the verified release output its first worker published after the requeue`). The worker renews a job's lease for as long as the workload's process group lives, not only its launching shell, so a build that outlives the shell that started it keeps its lease. The heartbeat log now says when a lease stops landing because the job left `running/` while the workload still runs, and when the workload's process group ends.

### Added

- **`stado web route` publishes a `cloudflare`-edge hostname through the Cloudflare tunnel:** it refused every such hostname and named `stado tunnel route` with item names to type, so a host behind a residential uplink, whose 80 and 443 nothing outside can reach, published nothing. The route now resolves the connector host and origin (a product's own host and loopback port, or the active host and endpoint of the service it fronts), reads the items playing `stado:role:cloudflare-api` and `stado:role:cloudflare-tunnel` in the owner vault, and runs the same ingress, connector token and proxied `CNAME` steps `stado tunnel route` runs; `--check` prints that plan and changes nothing. Refusals: a role no item plays (`no item in the owner vault on <owner> plays role cloudflare-api; tag the item … with stado credentials item retag …`), a mount or a redirect on the cloudflare edge (a tunnel route carries a whole hostname to one origin), a fronted service the directory does not declare or that has no endpoint on its active host, and a connector service not declared on that host. The zone must be served by Cloudflare first (`stado dns delegate <zone> --provider cloudflare`). `stado web remove` still leaves the tunnel route and names `stado tunnel remove`.

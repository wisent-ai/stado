# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.20](changelog/0.22.18-0.22.20.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado database destroy` is the inverse of `create` for every provider Stado creates with, read from the credential item. A Supabase database's hosted project is deleted through the management API, only with `--delete-project` (it takes every row with it); an external database is refused with `stado database remove`, which withdraws the declaration and leaves the server alone. Before, `destroy` treated every declaration as a fleet database: on a Supabase or external one it deleted the credential item and the declaration and left the project or server running with nothing pointing at it.
- The `--disk-cleanup` role of `com.wisent.stado` keeps running when a pass reports `invalid_or_unavailable_policy`. When the report has no `check_interval_seconds`, the watch reads its cadence from the host's registry declaration through the last-known-good copy. It ends only when the target declares no `disk_cleanup`.
- An unreadable settled-job run manifest leaves the job for recovery with `transition_run_manifest_unreadable`, while the remaining lifecycle pass continues.
- `stado build status` judges a platform from the job's retained receipt when the run reaper has retired its terminal job record, matching `release submit`.
- The release boundary validates the declared `release_api.publishers` table without requiring a compiled list of product names. `build submit` and `release submit` declare the submitted product's publisher when the host lacks it.
- The release agent's finishing pass uses retained receipts for retired build-job records, allowing a passed build to proceed to publication and delivery.
- A release run stays `delivering` while its delivery jobs are queued or running. The release agent's next pass, or `stado release resume`, collects their terminal results.
- A delivery whose job record the run reaper retired is judged by the outcome retained in its submission run manifest (`runs/run-release-delivery-*.json`), so `stado release resume` replaces a failed delivery instead of reporting that it never ended.
- A queue agent on the host that serves the object API reads a release input from the top of the served store. Rooted in the queue namespace, it resolved `stado://releases/...` under that namespace and failed every delivery to its own host with "input archive is absent" for an archive it was serving.
- A release run is judged once every delivery has a verdict: a required delivery that failed no longer fails the run while a sibling is still queued, which had made the sibling's worker refuse its job and withheld the release from every other host.
- `stado config migrate-identities` carries `agent.skarbiec.items` into `agent.skarbiec.roles` before retiring it; it used to drop the list, which left every `secret_fields` entry naming an undeclared role and the file failing its own validation. `stado release install-local` runs the incoming binary's `config migrate-identities` when that binary's `config validate` refuses the host, and installs only if the migrated file passes; before, every host still holding `agent.skarbiec.items` refused every Stado from 0.22.18 on.
- Stado's release recipes select platform-matched installation destinations from the registry. Host identities stay outside the source manifest, and each release retains its resolved placement for resume and redelivery.
- Breaking input change for 0.23.0: `registry validate` and `registry push` require an explicit source path; `registry push -` selects stdin. Missing input exits 2 even on a terminal and cannot select a bundled fleet document. Push failures name the selected input, while generation and empty-fleet guards remain in force. Desktop's Registry documents section exposes the same reads, validation, reviewed replacement and complete receipts. Replace bare invocations with the intended file or, for push, explicit `-` before upgrading.

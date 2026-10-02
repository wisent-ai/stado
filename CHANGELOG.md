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

- The `--disk-cleanup` role of `com.wisent.stado` keeps running when a pass reports `invalid_or_unavailable_policy`. When the report has no `check_interval_seconds`, the watch reads its cadence from the host's registry declaration through the last-known-good copy. It ends only when the target declares no `disk_cleanup`.
- An unreadable settled-job run manifest leaves the job for recovery with `transition_run_manifest_unreadable`, while the remaining lifecycle pass continues.
- `stado build status` judges a platform from the job's retained receipt when the run reaper has retired its terminal job record, matching `release submit`.
- The release boundary validates the declared `release_api.publishers` table without requiring a compiled list of product names. `build submit` and `release submit` declare the submitted product's publisher when the host lacks it.
- The release agent's finishing pass uses retained receipts for retired build-job records, allowing a passed build to proceed to publication and delivery.
- A release run stays `delivering` while its delivery jobs are queued or running. The release agent's next pass, or `stado release resume`, collects their terminal results.
- Stado's release recipes select platform-matched installation destinations from the registry. Host identities stay outside the source manifest, and each release retains its resolved placement for resume and redelivery.
- Breaking input change for 0.23.0: `registry validate` and `registry push` require an explicit source path; `registry push -` selects stdin. Missing input exits 2 even on a terminal and cannot select a bundled fleet document. Push failures name the selected input, while generation and empty-fleet guards remain in force. Desktop's Registry documents section exposes the same reads, validation, reviewed replacement and complete receipts. Replace bare invocations with the intended file or, for push, explicit `-` before upgrading.

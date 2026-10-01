# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.19](changelog/0.22.18-0.22.19.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- Numbers nobody chose are gone, together with what they drove. `doctor` no longer cuts a probe off after 8 s (or a multiple of it); each row answers when its dependency does. `billing watch --interval` and `host unit-log --lines` have no default and must be given. `config migrate` is removed with the config `schema_version` it existed to add; configs, the storage layout marker, autonomy records and resource plans no longer carry a schema stamp. Billing account health alerts on the first non-`ok` report instead of after an hour. An operator's resource plan no longer expires after a day and its operation lock no longer lapses after two hours (it is held until released); postconditions are read once after the provider operation finishes, and GCP operations are awaited through Compute's own `wait` method. Placement relief judges each host on its current publication (no 15-minute pressure window, no 30-minute cooldown). The optimizer prices by the calendar month and reads every feedback record; it no longer assumes a 120 s cloud startup or guesses GCP CPU/memory per shape. Release run listings read until the requested runs are found instead of stopping at 120. The Azure support ticket is polled on Azure's own `Retry-After`. Inference plan and route ids are the full SHA-256.
- A quarantine's cause is no longer guessed from English sentences in the candidate's log. The release agent names the cause where it observes it: a manifest without rollback compatibility is `rollback_compatibility_undeclared`; a candidate pid that is gone while another process listens on its port is `stable_bind_occupied`, and gone with the port free is `release_process_vanished`; a readiness request the HTTP client gave up on is `readiness_probe_unanswered`. A product's own `wisent-errors` envelope in the candidate's log still outranks what the agent saw. A log line without an envelope names no cause, so a record that only a sentence list used to classify now reads `unclassified`.
- No failure code is guessed from an error's wording any more. A failure's code is what the code that failed stated (`CmdError::failure`); one that states none is `unknown`, so only a stated retryable failure exits 69. `service ensure`'s registry read retries only a failure stated retryable.
- The service reconciler's row classification (`identity_unresolved`, `declaration_incomplete`, `repair_failed`) is stated by the repair that refused, not read from its sentence.
- A launchd stop or bootout treats exit status 3 or 113 (the job is not loaded) as the stopped state it asked for; the error text is not read.
- The Skarbiec recovery payloads exit 3 when the host was healthy and nothing changed; `host recover` and the memory pass read `recovered` from the exit status instead of looking for the word.
- `fleet enroll --install-key` no longer sorts ssh's exit 255 into "unreachable" or "rejected" by phrases; it says ssh could not open the session, quotes ssh's sentence whole and names both repairs. Stado Desktop's enrollment, invitation and adoption failures no longer pick a title by searching the command's sentence; they say what the step guarantees was left untouched and show Stado's sentence verbatim.
- `doctor`'s Skarbiec read-contract check passes on the broker rejecting an item-wide read as invalid (HTTP 400 or 422), not on the words `field required`; a 403 from Skarbiec always carries the grant-rebind repair; `quota azure-replies` and `azure-escalate` report az's own stderr instead of rewording a Forbidden they found by its words; a precheck runner target the registry cannot resolve is refused with the resolver's sentence.
- The cloud inventory no longer guesses `missing_permissions` from words like "forbidden" in a provider's error; the field is gone from inventory sources and plan details, and the provider's error is kept whole in `upstream_error`. A queue agent's Skarbiec grant diagnosis is chosen by the broker's 401/403 status, not by its sentence.

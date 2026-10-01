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

- A quarantine's cause is no longer guessed from English sentences in the candidate's log. The release agent names the cause where it observes it: a manifest without rollback compatibility is `rollback_compatibility_undeclared`; a candidate pid that is gone while another process listens on its port is `stable_bind_occupied`, and gone with the port free is `release_process_vanished`; a readiness request the HTTP client gave up on is `readiness_probe_unanswered`. A product's own `wisent-errors` envelope in the candidate's log still outranks what the agent saw. A log line without an envelope names no cause, so a record that only a sentence list used to classify now reads `unclassified`.
- No failure code is guessed from an error's wording any more. A failure's code is what the code that failed stated (`CmdError::failure`); one that states none is `unknown`, so only a stated retryable failure exits 69. `service ensure`'s registry read retries only a failure stated retryable.
- The service reconciler's row classification (`identity_unresolved`, `declaration_incomplete`, `repair_failed`) is stated by the repair that refused, not read from its sentence.
- A launchd stop or bootout treats exit status 3 or 113 (the job is not loaded) as the stopped state it asked for; the error text is not read.
- The Skarbiec recovery payloads exit 3 when the host was healthy and nothing changed; `host recover` and the memory pass read `recovered` from the exit status instead of looking for the word.

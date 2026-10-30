# Changelog Unreleased (part 15)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **Adding a machine to the fleet declares every way it fails:** `stado fleet enroll`, `stado
  fleet join` and `stado fleet approve` raise only declared causes (`fleet.enroll` 57, `fleet.join`
  10, `fleet.approve` 63), each naming what failed, why, the state observed and what fixes it, with
  its code and exit status: the registry read and write by the class the store answered, the
  enrollment catalog's gates with the methods the fleet does allow, the channel key, the
  `--install-key` session, each way OpenSSH ends the probe (unresolvable, refused, timed out, no
  route, key refused, host key changed), the platform, the install and its rollback, and the
  network join. A failure prints `fix: ...` under its `Error:` line and names its cause; `--json`
  answers it as JSON with `cause` and `remedy`.
- **`stado errors list|show|check`:** `list` names the operations that declare a failure catalogue,
  `show OPERATION` prints each cause with its fix, code, exit status and test (`--json`,
  `--markdown` for the documentation page), and `check` is the new `error-standard` quality gate:
  it refuses a catalogue that is not one and an error message holding a generic sentence ("did not
  succeed", "operation failed") beyond `stado-rs/errors-baseline.json`, which may only shrink.
- **The submit queue runs by itself:** the Stado CLI installation declares a host routine
  (`stado schedule create --host-store`, every ten minutes, overlap skipped) running `stado change
  run` with no checkout, which carries every repository of this machine's queue that has a
  candidate waiting, each from the checkout its candidate was submitted from. Nobody ran the pass
  before: it ran inside agent sessions, whose end killed it, its reviewer and the candidate it
  carried. `stado schedule create --host-store` declares any routine into this host's own store.
- **Each candidate without a verdict says what it waits on:** `stado change list` and `status`
  print `waits on: <the next pass | a pass | the remote | Oko's admission of its gates | a host for
  its gates | a harsh review round>` with the command that unblocks it, and `--json` carries it as
  `waits`; the record keeps the step as `waits_on`.
- **`stado change review ID --findings FILE --improvements FILE`:** records a review round over a
  queued or running candidate beside its record (`<id>.rounds/`), by the agent session the command
  runs in as its harness names it (`tama sessions self`, else the transcript `PI_SESSION_FILE`
  names), never a name typed; the submitting session and a command no session runs are refused. The
  round is over the commit the next pass lands (the candidate as the last pass wrote it on `main`)
  and keeps its tree: a round over a tree a moved `main` replaced decides nothing. At the review
  before landing each reviewer's newest round over the tree counts, so one reviewer never clears
  another's findings: any finding refuses the candidate naming it, and rounds that all found nothing
  admit it. Tama's harsh review is one adapter, run only when no round is recorded; without Tama
  the candidate waits on `a review round over commit <sha> by a session that is not its author`,
  naming this command.
- **A host whose worker cannot reach the fleet store says why in `stado host gates`:** every
  capacity write, every agent start and every tick that met a store that did not answer leaves its
  outcome on the host in `~/.stado/logs/capacity-publication.json` (step, time, the store's words,
  since when it refuses, the Stado version). `host gates` reads it over the host channel, which needs
  no store, and prints `its worker cannot reach the fleet store: its <step> at <time> was refused,
  refused since <time> (Stado <version>): <error>` where it said only `nothing published`; a host
  with no record is named as one whose worker never ran under a Stado that keeps one. `--json`
  carries it as `publication_attempt`.

### Changed

- **Enrollment refuses a request's form before reading anything:** a name that is not a lowercase
  target identifier, an `--ssh` destination ssh would read as an option, and a `--kind` nobody
  declares are refused with exit 2; `approve` refuses a fleet nobody declared before writing.
- **A stored join request that is not JSON is `config`, exit 1,** not a retryable outage (exit 69):
  retrying never repairs it; `stado fleet reject` and a new `join` do.
- **A candidate left running by a pass that is gone is resumed and named:** a pass records itself
  on the candidate it carries; a running candidate whose repository's pass lock is free (the lock
  the kernel releases when the pass's process ends, killed or not) is listed as `waits on: a pass;
  the pass that carried it (pid N) is gone …`, and the next pass resumes it.
- **The resident pass asks Oko as the fleet's daily build:** a pass under no agent session asks
  `oko admit compile|test` with no session first, the run the operator's `@fleet-budget` consents
  admit, then as the submitter's work; its refusal names the subject that would admit it
  (`<action>:<product>@fleet-budget`). A pass inside a session still asks as the submitter only and
  says so.
- **A fleet store that does not answer no longer stops the queue's gates:** when the hosts'
  capacity publications cannot be read, this host runs the gates of its own platform and the
  registry's hosts of the other platforms run theirs unjudged, each said with the store's error, so
  a repair of the fleet store can land through the queue.
- **A candidate stacked on a waiting candidate is measured over it:** `stado change submit`
  measured a commit's size from its merge base with `main`, so a change stacked on a queued one was
  refused for the lines below it, which that candidate had already been measured for. A commit
  that holds a queued or running candidate's commit `main` does not is now measured past the
  nearest such candidate, and the refusal names it: `<commit> changes <n> lines in <m> files past
  candidate <id> it is stacked on (<its commit>), more than …`.

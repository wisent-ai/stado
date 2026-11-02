# Changelog Unreleased (part 19)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Changed

- **A model request is handed to Stado, and Brama's alternative is a statement recorded for the
  request:** a product resolved `inference`, then dialled what it was handed itself, reading the
  endpoint's key and choosing whether to sign, so every alternative added to the catalog needed a
  change in every product. `stado capabilities call inference --consumer C [--json]` takes the
  OpenAI chat-completions request on standard input, asks the first alternative in the selected
  order that answers it and prints that answer; one that cannot answer this request is named with
  its cause and the next is asked. Brama is sent the request with the `token` of
  `<consumer>-model-router` and, when an item plays `<consumer>-agent-auth`, its signature. The
  OpenAI-compatible `model-endpoint` alternative and its `capabilities.inference.endpoint.*`
  settings are removed; `attestation` takes its place: a request no alternative before it
  answered is kept under its SHA-256 and refused naming the command that answers it, and
  `stado capabilities attestation list|show|record|revoke inference --consumer C` reads it and
  records the answer with what was checked, answering that exact request as a chat completion
  whose `model` is `attestation`. Stado Desktop runs the same operations on the Compute providers
  screen.

### Fixed

- **No check formats or lints a tree:** `stado quality check --pre-push`, `stado release changes
  submit` and the delivery queue's gate step check the committed source alone (lock resolution,
  module declarations and reach) and run no gate; a push to `main` no longer runs `lint` gates on
  fleet hosts and asks Oko no admission, and `--host` on `--pre-push` is gone with it. The full
  `stado quality check`, release submission and `stado change run` run only `test` gates; a
  `format` gate runs only under `stado quality format`, which writes. Whether code compiles is
  learned from reading it and from the tests that compile it. A refused web gate is named as a
  web gate whatever its kind, and the source-only delivery check that ran beside the format
  gates is gone with them.
- **`stado product install` and `update` build without formatting, linting or checking first:** a
  source install ran every quality gate its platform declares (`cargo fmt --check`, `cargo clippy`,
  `stado errors check`) before the build step, recompiling a revision the push had already checked
  and holding the install for many minutes, and a recorded gate failure refused the revision before
  it was exported. An install now exports the committed revision, builds and places it; whether the
  code compiles is learned from its tests and from reading it, and a revision that does not build
  fails the build step naming its evidence. The `install-failures` records are no longer written
  or read, and the next install of a checkout removes the ones an earlier Stado kept in its build
  area. Stado's own manifest declares no `format` or `lint` gate any more (`fmt`, `clippy`,
  `error-standard`), so no push, queue pass or release of Stado runs them either; its test gate
  stays.
- **`stado release submit`'s CLI audit no longer runs a declared launcher:** the member a release
  manifest's `runtime.launcher` names (`bin/start`) is started by release control with the binary,
  port and runtime directory it assigns, so the audit no longer reads its `--help`. Run bare,
  Skarbiec's launcher exited 1 on `SKARBIEC_BIN` and refused the release.
- **`stado release submit` carries a published diff that is not UTF-8:** a committed binary file git
  reads as text (Skarbiec's compressed inflate test vectors) put bytes that are not UTF-8 into the
  diff the source archive records for its gates, and the submission was refused with `answered
  non-UTF-8`. The record now carries exactly the bytes `git diff` gave, as `diff` when they are
  UTF-8 and as `diff_base64` when they are not, and its `sha256` is the digest of those bytes, so
  it can be checked against `git diff` itself.
- **A placement move carries every state as a file, and copies it before the fence:** a file state
  travelled as base64 inside the read script's output and the write script's heredoc, held whole
  in the move's memory with the source fenced for the whole copy, so a state of a few gigabytes
  held its service down for hours and then failed with `bash: xrealloc: cannot allocate`.
  Every state, file or directory, now goes through `rsync -a` into a staging copy on the machine
  running the move and on to a stage beside its path on the destination, renamed over it after the
  previous file or tree is renamed to the backup. The preflight and a first copy of each state
  (`copy <state> while it serves`) run before the move claims its transaction, since an open
  transaction refuses the profile's services to every consumer; the claim is then laid on the
  registry as it stands and refused when the profile moved meanwhile. After the fence each state
  is read again by content (`read what changed in <state>`), so the services are refused for what
  changed in between; a role move copies its store the same way before it arms the store fence.
  The staged copies are removed once installed, when the move rolls back and when it fails before
  its claim, and the local staging room counts every state.
- **A Stado built from an exported tree no longer claims the enclosing checkout's revision:** the
  build asked `git rev-parse`, which walks up to any enclosing work tree, so a tree exported inside
  another checkout reported that checkout's HEAD with `-dirty`. Git is now asked only when the
  package's parent is the root of its own work tree; otherwise the build embeds the revision
  `STADO_SOURCE_REVISION` states, or `unknown`.
- **A move of the fleet vault can commit:** the move stopped the vault's unit as its fence, and
  the registry authority asks that vault for its store bearer's version on every write, so a move
  of the vault off the authority's host copied every state and then failed at `commit the
  placement` and rolled back. The profile carrying `skarbiec` now holds the vault's writes with
  `skarbiec fence arm --transaction <T>` (step `hold the vault's writes`), so every read keeps
  answering through the commit. After the commit the source host's resolver is asked to follow
  and the source's units are retired, then the retired copy's fence is lifted so a later move back
  finds no fence standing; each of those that fails is listed under
  `unretired_source` and does not roll back a committed move. A move that fails before the commit
  lifts the fence instead of restarting units that never stopped. The preflight refuses a source
  whose Skarbiec has no write fence, a source whose `skarbiec` unit does not run that Skarbiec or
  whose own `registry doctor` reports the unit's process as not executing its installed file (an
  old process ignores the fence on its writes), and a source whose own Stado reads the vault at
  the unit the move retires, naming the configuration that sends it through its resolver adapter
  instead.
- **The fleet vault keeps resolving while its own move is open:** the resolver refused every
  service of a profile under an open placement transaction, so once a host's registry authority
  and object API read the vault through their resolver adapter (`secrets.skarbiec.url
  stado://service/skarbiec?consumer=stado`, what the vault move's preflight asks for), the move's
  own claim cut them off from the vault every store write is checked against: the claim landed,
  its receipt never came back (`received unexpected message from connection` from the adapter's
  502), and the transaction stayed open with the vault refused to everyone. `skarbiec` now
  resolves to its active host, the write-fenced source, until the commit retargets it; every
  other service of a moving profile is refused as before.
- **A move whose commit landed is not rolled back when its receipt is lost:** the registry
  authority writes the commit and then answers, and its answer can fail after the write (a vault
  move's authority loses the vault for a moment once its resolver follows the commit). The move
  took that as a failed commit and rolled every host back under a registry that already placed
  the profile on the destination, so every consumer was sent to a host that no longer served it.
  A failed `commit the placement` now reads the registry back: this move's transaction released
  and its services active on the destination is a commit, and the move goes on as committed; a
  registry that does not hold it rolls back as before; one that cannot be read leaves the hosts
  as they are and says how to tell (`placement transactions`: the transaction still open means it
  did not land, and `placement release` undoes it).

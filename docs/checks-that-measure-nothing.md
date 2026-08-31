# Checks that measure nothing

One defect shape has now been found twenty times in this repository, in
twenty different subsystems, inside about fourteen hours. The twentieth was
mine, in the diagnosis of the nineteen. Every instance is
the same thing: **a declaration checked against something narrower than the world.**

The check passes. The declaration is self-consistent. Nothing compares it to
what is actually there.

## The twenty

| # | Where | The declaration | What nothing checked |
|---|---|---|---|
| 1 | `FLEET_LABEL_PREFIX`, `src/deploy/service.rs:4538` **and** `src/deploy/local_install.rs:41` | a unit label is built by prefixing a name | that the name did not already carry the prefix. Read the fix: `67a5d995` repaired `label()` and, in the same diff, **added the second definition of the constant** — so the repair for a duplicated prefix created a duplicated source of truth for the prefix. Both are still `"com.wisent."` at `main`, one `const`, one `pub const` |
| 2 | `src/deploy/host_disk.rs`, `src/deploy/host_gates.rs`, `src/providers/local/disk_cleanup/mod.rs` | a host is above its disk watermark | that the janitor and `host gates` read the same source; they did not |
| 3 | `mirror_to_output_uri`, `src/providers/local/slots.rs:625` (#161, #169) | an output URI is a key | qualified store path and bare key are both `String`, so callers guessed — 417 objects landed at `ecosystem/<ns>/ecosystem/<ns>/…` |
| 4 | backup backend config (#166, #168) | replication is off for this host | that a second, never-audited writer kept filling the replica — it reached 48.5 GiB |
| 5 | `.github/workflows/deploy.yml` publish loop (#163) | the publish loop returned cleanly | that every declared binary is present — `stado/0.11.0/darwin-arm64` holds 4 objects of 9, permanently |
| 6 | `src/cli/status.rs:96` (#175, **open**) | jobs are "extracted (awaiting upload)" | that `completed` is terminal and `uploaded` is a different state — 52 finished jobs read as a jammed queue |
| 7 | `scripts/surface.py` + AutoVersion (**open, unfiled**) | the declared version matches the change | that the *tree* changed; the gate measures the advertised command surface only |
| 8 | `.github/workflows/writer-transfer-check.yml` (#173, #174) | the newest `stado-v*` tag names published bytes | that the coordinate exists — a tag is created before publication and survives one that never completed |
| 9 | `Presence`, `src/cli/storage.rs` (#174) | a failed `storage get` means the object is absent | that the store answered at all — "absent" and "unreachable" need opposite responses |
| 10 | the object API's `/healthz` | the service is `"ok": true` | that any capability works. Measured 2026-08-31: `{"ok":true,"degraded":true,"boundaries":{"object":false,"release":false,"service":false,"machine":false,"integration":false,"rate_limit_verifier":false,...}}` while `/api/object` timed out and every authorized route returned 503. **Detected, not merely noticed:** check 3 of the fleet-shape detector (#181) reports this on the tick |
| 11 | `boundary_timeout` in the object API's authorization validation (#181) | 90 seconds is enough to validate every boundary | anything about how much work that covers. The budget is flat; the work is not. `charless-mac-mini` declares 17 object namespaces, 14 release publishers and 4 service deployers, so the object boundary alone needs 18 sequential vault decrypt-and-audit operations — deliberately serial, because fanning out caused resets — and all six boundaries race one single-threaded vault inside the same flat 90 seconds. Every component was healthy; the fleet had simply outgrown the number. `doctor::object_auth_deadline` had already solved this arithmetic one module away by budgeting per declared item, and the boundary the whole fleet reads through never got it |
| 12 | `host_inventory`'s listener collector (#185) | the table of listeners shows who holds a port | that it can represent more than one holder. It dropped every non-loopback address and kept only the first row per port — so a port with three servers on it looked like a port with one. **This is the sharpest instance in the table, because it is inside the detector built tonight to catch this pattern:** the fleet-shape check counted holders through an instrument that had already removed the evidence. A check built on a blind instrument is the same as no check |
| 13 | `WC_BACKUP_STORAGE_BACKEND=local` in the object API unit (#168) | replication is the coordinator's tick, and it is off | that the store itself mirrors every object it accepts into a bare path beside itself. The replica had a second writer nobody was looking for, which is why the twins reappeared at almost exactly 15 GiB each time instead of growing — they were not left over, they were being remade |
| 14 | `Boundary::Release` in `src/dashboard/mod.rs` | a boundary named "release publication" reports whether release publication can work | that anything consults it. It is enumerated (110), labelled (123), described (136), branched (244) and validated once at startup (550) — and required by **no route in the tree**. `boundaries_available` revalidates a closed boundary only when a request needs it, so once it fails at startup it reads `false` forever and gates nothing. It nearly cost the only quiet publishing window of the night, because the honest reading of a field called "release publication" is that publication is broken. Two honest resolutions: the release routes require it, or it stops being reported as a boundary |
| 15 | the `doctor` preflight in `deploy/deploy_stado_rust.sh` (#181, being fixed) | the fleet-shape finding names what an operator should declare | that the operator *can* declare it. The remedy says to add `queue_workdirs` and `backup_twins` "AFTER the host runs a binary that knows it" — and the check then fails the preflight that delivers that binary. A gate whose precondition is the outcome the change enables. Same shape as instance 8, found the same night, in the opposite direction |
| 16 | `validate_registry` and every write path (#197) | this document is valid | which *section* is not. One unresolvable `inference` entry refused the entire document, and `declare-version`, `promote-version`, `service adopt`, `host add` and `registry push` all validate the whole document before writing — so a field those writes never touch could freeze every domain at once. The blast radius was the fault, not the value |
| 17 | **our own binaries**, and then the fleet's | the tool reading live state understands the code that produced it | its own age — and this is not the false positive it first looked like. `stado registry validate` refused `inference.routes["wisent-backend/evaluation"] = "best"`; that value is permitted by `gateway_selector` (`src/inference/schema.rs`, `value == "best"`), added in `f020b63e` at 05:56:02Z. Two agents read the refusal from binaries built before it and concluded the fleet's registry was unwritable. **But `f020b63e` landed 3m43s AFTER `stado-v0.13.9` was tagged, so it ships in 0.13.10 — and every host below that, including the one just delivered 0.13.9, genuinely cannot parse `"best"`.** At 07:13:43Z the mini's janitor answered `invalid_or_unavailable_policy`, `errors: ["policy:ValueError"]`, `target_name: null`: it rejected the WHOLE registry over that one route, and rejecting the document means resolving no `disk_cleanup` policy at all — so every cleaner on the host stopped, including the `build_caches` armed hours earlier. Restoring the route to `openai/gpt-4o-mini` at 07:19:11Z returned it to `errors: []`, `mode: enforce` by 07:25:20Z. One entry in a section the janitor never reads was a fleet-wide janitor kill switch for any host below 0.13.10. Found by `store-reclaim` |
| 18 | `host disk`'s `outcome`, read from `~/.cache/wisent-compute/disk-cleanup-state.json` (#206) | the janitor's state file says whether disk maintenance is working, and when it last ran | **which janitor wrote it.** Two processes write that path on an always-on host — the queue agent in-process every tick (`providers/local/agent.rs:783`) and the standalone launchd unit `com.wisent.compute.disk-cleanup.disk-cleanup` on its own timer (`cli/disk_cleanup.rs:48`) — and `host disk` reported whichever wrote last as the state of the host. Measured 46 seconds apart: `interval_noop`, `errors: []`, all six cleaners scanned, then `invalid_or_unavailable_policy` from the same path. Both true about their own writer, neither true about the host. **The codebase already knew.** `deploy/host_gates.rs:324-341` documents this exact file's multiple writers — "the queue agent every ten seconds, a `disk-cleanup --watch` unit on its own timer" — because it had already been bitten by it, reading `low watermark 20 GiB, target 18 GiB`, a floor above its own ceiling, alternating with the canonical 15/18 between one reading and the next. It fixed *itself* by preferring the registry declaration and left the other reader of the same file unfixed. **And the cost is not only reporting.** `run_with_lock`'s interval gate reads `last_attempt_at` from that same shared file (`disk_cleanup/mod.rs:1289-1303`) and returns `interval_noop` *before* scanning a single cleaner and *before* the lock is reached — so the redundant unit's stamp starves the real janitor. Proven under manufactured pressure by `store-reclaim`: thresholds raised to 40/42 GiB with 31.2 GiB free, and at 15:26:03.9Z the agent reported `disk_pressure_active: true`, `errors: []`, and every cleaner `scanned 0`; 27 seconds later the other janitor stamped the file `invalid_or_unavailable_policy`, `next_pass 15:31:31Z`. Pressure active, policy resolved, nothing scanned. Not a check that missed something: two instruments reporting on one subject with no arbitration — so the operator's verdict depends on timing, and the redundant one disables the real one |
| 19 | `src/cli/mod.rs` and every `mod` declaration in the crate (#212) | the files in the tree are the code that runs | **that anything declares them.** A `.rs` file no `mod` declaration names is not a compile error - it is not compiled at all, and `cargo check`, `cargo clippy`, `git log` and a file listing all look untouched. On 2026-08-31 at 15:48:36Z a commit replaced `src/cli/mod.rs` with a six-day-old copy, deleting nine `pub mod` declarations - `builds`, `database`, `egress`, `fleet`, `product`, `release_evidence`, `release_quarantine`, `service_converge`, `stream` - while all nine files stayed on disk. `main` did stop compiling, but 27 errors away in unrelated callers, and **no diagnostic named a missing declaration**; the whole `cli/fleet/` subtree went dark and the symptom was `dashboard::run` being called with two of its three arguments. Fourteen seconds later a second commit added a 279-line `src/bin/stado_fleet/key/mod.rs` to fix `stado fleet key ls`, and nothing declares it: that fix has never been compiled, while the command it corrects still lists by item-name prefix at `src/cli/fleet/key/mod.rs:270` - against its own commit message's principle that behaviour must never be derived from item names. A third file, `src/queue/secrets.rs`, has been product code compiled into nothing since 2026-07-27. **Now checked:** `stado-rs/scripts/unreachable_modules.py` resolves every declaration from `src/lib.rs` and each `[[bin]]` path and fails on any file the walk never reached - 3 unreachable at the healthy tip, **23 at the clobbered one** - wired into `version-check` before anything is built, with a ratchet file recording the four known ones and their reasons |
| 20 | **our own diagnosis**, and the guard I nearly shipped for it (`configured_object_base_url`) | a `*.ts.net` MagicDNS name resolving to a public address means the name has been hijacked | **that anything checked WHO answered.** On 2026-08-31 `charless-mac-mini.tail6443b3.ts.net` resolved to `208.111.34.11`, `208.111.35.209`, `2607:f740:0:3f::2f0`, `2607:f740:0:3f::3cc` — public addresses, on two workstations, through `getaddrinfo` and not just `host`. Two agents independently concluded that MagicDNS was answering for a stranger and that any caller on that name would leave the tailnet with its bearer token. I wrote the refusal into `configured_object_base_url`, the single gate every origin passes: any `.ts.net` name resolving outside `100.64.0.0/10` / `fd7a:115c:a1e0::/48` is refused. It compiled, and it correctly refused the name with all four addresses listed. Then the one check nobody had made: `openssl s_client` returns **`subject=CN=charless-mac-mini.tail6443b3.ts.net`, issuer Let's Encrypt**, and `whois 208.111.34.11` is **NetActuate, Inc** — the provider Tailscale runs Funnel ingress on. A `*.ts.net` name resolving to a public address is Funnel working exactly as designed: the ingress terminates for that exact name with that node's own certificate, which no stranger can present. **The guard would have refused every Funnel origin in the pipeline** — `STADO_PUBLIC_RELEASE_API_URL` at both publish legs, `deploy-existing-release.yml`, and `version-check.yml`'s `STADO_API_URL` — breaking publication entirely in the name of securing it. Deleted unpushed. `IP is in the tailnet range` is narrower than `the peer is the one named`; TLS already checks the second, and `configured_object_base_url` already requires HTTPS for every non-loopback origin. The first instance here where the suspected wrong answer was a live network peer, and the answer was the right machine all along |

## The property they share

In each case the system stored an *intent* and then re-read its own intent as
evidence. A name assumed to lack a prefix. A watermark read from one place and
enforced from another. A key assumed to be bare. Replication assumed off
because config said off. A publish assumed complete because the loop exited. A
tag assumed to mean bytes exist. A timeout assumed to be enough. A port
assumed to have one holder, counted with a tool that could not show two. A
boundary assumed to gate what its name says. A binary assumed to be current. A
state file assumed to have one writer. A file on disk assumed to be code. A
name assumed to be hijacked because its address was public.

Several of these passed a validator, a schema or a health check first, and one
of them passed a fix aimed at that very defect. So: **a defect that survives a
check tells you the check models the wrong thing.**

## What it cost

Four coordinates, permanently. Release objects are create-only and immutable,
so none of these can ever be completed:

- `stado/0.10.0/darwin-arm64` — 0 objects of 9. Never publishable for macOS.
- `stado/0.11.0/darwin-arm64` — 4 of 9.
- `stado/0.12.1/linux-amd64` — 2 of 9: `SHA256SUMS` and the manifest, no binaries.
- `stado/0.13.2/darwin-arm64` — 1 of 9.

Eight version numbers were spent in one night before anything published whole,
and the fleet had no deliverable macOS build between 0.9.5 and 0.13.9.
Separately, three queue agents published capacity for one consumer id under
labels the registry never declared, and 55 pinned jobs were refused for
seven days by a process no report could name.

## What a check has to do to be worth having

1. **Measure the world, not a restatement of the intent.** Read the objects
   back, list the listeners, stat the coordinate.
2. **Never let "I could not tell" collapse into "no".** `storage stat` answers
   `present`/`absent` with exit status zero and reserves non-zero for a store
   that did not answer. `BlobBackend::exists` cannot express that difference,
   which is why it is not used here.
3. **One declaration, used twice.** Generate the artifact and assert against
   the same list, in the same file, so the check cannot drift from what it
   guards.
4. **Ask what validated it.** If a schema or health check passed over the
   defect, that check is the next thing to fix.
5. **Check the instrument, including the binary.** A negative result from a
   tool older than the code it is reading is not a finding. But instance 17 is
   the harder lesson: a stale-instrument artefact and a real version-dependent
   defect look identical from outside and have opposite consequences, so
   "our tool was old" is where the investigation starts, not where it ends.
6. **Fixing one reader of a shared artefact is not fixing the artefact.**
   `host_gates.rs` had already been bitten by the janitor's state file — it
   read a 20 GiB floor above its own 18 GiB ceiling, alternating with the
   canonical 15/18 between one reading and the next — and it documented the
   multiple writers in full before preferring the registry declaration. It
   repaired itself and left `host disk`, reading the same file, to report the
   losing writer's verdict as the host's for months. The same shape as
   `doctor::object_auth_deadline`, which solved the boundary arithmetic one
   module away from the boundary that needed it. When the diagnosis is written
   down and the repair is local, the next reader inherits the defect **and**
   the comment explaining it. Fix the artefact: make it say who wrote it.
7. **A green build is not evidence the tree is wired up.** Rust reports a
   missing `mod` declaration only where something references the module, so
   nine deleted declarations surfaced as 27 errors in unrelated callers, and an
   undeclared file that nobody references surfaces as nothing at all. The check
   has to enumerate the files and resolve the declarations - the same
   one-declaration-used-twice rule as #3, applied to the module graph.

## What was proven, on live runs

Four claims from this night have evidence behind them rather than an argument.
Everything else in this document is a defect; these are the repairs that were
measured working.

- **Per-tag release concurrency.** The `stado-v0.13.9` train ran **62 minutes
  uncancelled**. The four before it died in two to five, each superseded by the
  next tag through one shared group.
- **Platform decoupling.** On that same run `publish-linux` failed and
  `darwin-arm64` published whole anyway — impossible under the previous
  `needs: release` graph, which is how 0.10.0 and 0.12.0 lost the platform.
- **Archive-first ordering.** Every failed train this night left its coordinate
  **empty rather than partial**: 0.13.0, 0.13.1 and 0.13.9's linux leg all
  measured 0 of 9. An empty coordinate is retriable; a partial one never is.
- **Where the partials actually came from, since this gets misremembered.**
  Publication completes *before* public validation runs: when
  `Validate public … release delivery` failed for 0.12.0 the linux coordinate
  was already 9 of 9. So a failure in that step costs a red job and a WHOLE
  coordinate. Every permanent partial came from a publish loop dying mid-write,
  which is what the bounded retry and the read-back assertion address. Nobody
  should hold a train because the validation step might fail.
- **What actually closed the janitor loop**, in the order it happened, because
  three separate causes each looked like the whole problem. `0.13.13` published
  **9 of 9 on both platforms** — the first complete coordinate carrying the
  per-writer gate, and the first train since `0.13.11` where both platforms
  published whole. It was delivered to `charless-mac-mini` and confirmed
  independently: `service converge` reads `DECLARED 0.13.13 / INSTALLED 0.13.13
  / in-sync`, on a release-profile binary because the debug profile of that tip
  cannot run any `stado service` subcommand without overflowing its stack.
  Before that, the rogue writer was identified **by removal**: the user-domain
  copy of `com.wisent.compute.service.stado-local-control-plane` was holding a
  five-day-old `stado dashboard`, it was booted at 18:26:24Z, and the
  `invalid_or_unavailable_policy` stamping stopped after one final write at
  18:29:42Z. And the restart condition written into the `PROCESS differs` line
  fired for the first time on its own terms — not "restart it because this line
  printed", but "a fix that this process executes has been delivered", which
  became true the moment 0.13.13 landed and the coordinator was still executing
  0.13.9. Installing is not executing.
- **The completeness gate.** `stado host release --version 0.13.9 --dry-run`
  refused while the coordinate was short, naming the absent objects, and then
  correctly stopped refusing once darwin reached 9 of 9 — advancing to a
  different and correct refusal about the registry declaration. A gate that
  only ever says no has not been shown to work.

## Loose ends, kept visible on purpose

A record that quietly drops these is the thing it was written to prevent.

- **`stado-v0.13.7`'s cancellation is unexplained.** 05:19:34Z, a hosted
  `ubuntu-latest` job, 78 seconds before `stado-v0.13.8` existed, with 214 GiB
  free on the self-hosted host. Neither disk exhaustion nor tag concurrency
  covers it. Per-tag concurrency has **not** absorbed it. One residual
  canceller may still be real, though the 0.13.9 train surviving 62 minutes is
  the first evidence against a persistent one.
- **The four permanent partials above stay on the list.** They are not
  historical trivia; they are the reason every check in this document exists.
- **No write path in the pipeline depends on a MagicDNS name**, established by
  audit rather than assumed: every publish write goes to
  `http://127.0.0.1:18776`, and the four `*.ts.net` sites
  (`deploy.yml` at both publish legs, `deploy-existing-release.yml`,
  `version-check.yml`) are read-backs only, each either byte-compared with
  `cmp` against the local source or digest-verified against the release
  manifest. So even under the misdirection of instance 20 — which turned out
  not to exist — a wrong answer could not have substituted release bytes. The
  property that the fleet cannot be answered by a host other than the one it
  named is established by TLS for the exact name, which
  `configured_object_base_url` already requires for every non-loopback origin.
- **One runner serialises every publication, and that is a capacity limit
  rather than a defect.** In `deploy.yml` both publish jobs -- `publish-linux`
  and `deploy-control-plane` -- declare
  `runs-on: [self-hosted, stado-control-plane]`, and exactly one runner carries
  that label (`lukasz-macbook`). Only `qualification` and `identity` can run
  hosted. Measured on 2026-08-31: a `product release` job took the runner at
  16:54Z, and from then until at least 18:05Z BOTH the 0.13.13 and 0.13.14
  trains sat with `publish-linux` and `deploy-control-plane` queued and no
  runner assigned -- four publish legs across two trains, plus product-release
  work from other sessions competing for the same label. 0.13.13's linux leg
  finally started at 18:03:23Z, an hour and sixteen minutes after its train
  began. Per-tag concurrency stopped trains from cancelling each other; it
  cannot make them run in parallel. The honest fix costs hardware, so it is a
  decision to take in daylight with a budget, not during a release. Its first
  cost is already recorded: a fleet queue held paused for 65 minutes waiting
  for a publication window that never arrived, with 18 jobs waiting.
- **`0.13.9/linux-amd64` is deliberately empty.** `publish-linux` needs the
  service and release boundaries, and instance 14 is the release boundary.
  Retrying it now would risk a fifth permanent partial to satisfy a gate that
  gates nothing.

## Open

**#7 is unfixed and unfiled.** `version-check` compares the advertised
command surface against the published channel. Both `dbb7e27e` and `375955a6`
declare `0.13.1`; they differ by a workflow fix and a chunked-upload change in
`src/cli/storage.rs`, and both advertise the same 49 top-level commands. So
autoversion reads the change as `internal` and the same version number stands
for two materially different trees. A published `0.13.1` would name bytes that
are not the bytes on `main`, and nothing in the gate can see it.

Fixing it changes the shared release rule
(`https://github.com/lbartoszcze/AutoVersion`) and what every future PR must
declare. That is a decision to take deliberately, not during an incident.

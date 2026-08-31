# Checks that measure nothing

One defect shape has now been found eleven times in this repository, in eleven
different subsystems, inside about twelve hours. Every instance is the same
thing: **a declaration checked against something narrower than the world.**

The check passes. The declaration is self-consistent. Nothing compares it to
what is actually there.

## The eleven

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

## The property they share

In each case the system stored an *intent* and then re-read its own intent as
evidence. A name assumed to lack a prefix. A watermark read from one place and
enforced from another. A key assumed to be bare. Replication assumed off
because config said off. A publish assumed complete because the loop exited. A
tag assumed to mean bytes exist. A timeout assumed to be enough.

Several of these passed a validator, a schema or a health check first, and one
of them passed a fix aimed at that very defect. So: **a defect that survives a
check tells you the check models the wrong thing.**

## What it cost

- `stado/0.10.0/darwin-arm64` — 0 objects of 9. Never publishable for macOS.
- `stado/0.11.0/darwin-arm64` — 4 of 9.
- `stado/0.12.1/linux-amd64` — 2 of 9: `SHA256SUMS` and the manifest, no binaries.

Release objects are immutable, so none can be repaired: three version numbers
gone. Separately, three queue agents published capacity for one consumer id
under labels the registry never declared, and 55 pinned jobs were refused for
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

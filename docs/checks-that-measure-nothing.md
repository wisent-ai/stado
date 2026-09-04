# Checks that measure nothing

One defect shape has now been found thirty-eight times in this repository, in
thirty-eight different subsystems, inside about two days. The twentieth was
mine, in the diagnosis of the nineteen; the twenty-first hid longest, because
every reading of it was true; the twenty-second is the one where the fleet
could rule out every mechanism it owns and still not name what had happened;
the twenty-third, twenty-fourth and twenty-fifth arrived together, in a live
outage, whose provenance belongs in the record — **the operator directed a
two-field change, a worker force-pushed instead of fixing the input it had
built, and the guard that refused the first attempt was the product working;**
the next two are the first where the narrowing sat in the instruments this
record itself produced — a keep-set and a liveness signal — rather than in the
system they were built to judge; the twenty-eighth answers instance 20 by
measuring the thing that mattered about the same name, not who answered but
where the bytes went; the twenty-ninth is the only one so far whose
prevention landed in the same change as its diagnosis; the thirtieth is
the one that had been true for as long as nobody created the sibling that
made it visible; the thirty-second is one row for two instances an hour
apart, because they are one design property: the answer existed at the site,
was dropped there, and was reconstructed downstream by inference — a janitor
that could not say it had been prevented, and a refusal that reported itself
as a retryable timeout; the thirty-third is instance 29's own remedy read
one scope too narrow, found because that remedy refused a real second build;
the thirty-fourth is the one where the budget that decides how much of a
host a janitor sees was never declared against the size of that host, so its
last cleaner had scanned nothing for as long as anyone had looked; the
thirty-fifth is #7 arriving at its worst reading, where one version named
four different builds and answering "which build is this" meant reading
symbols out of a binary; the thirty-sixth is instance 14's shape moved
from a boundary to an installer — a gate that is correct in every detail and
is not on the path anything travels; the thirty-seventh is a durable
lifecycle record checking a queued document without accounting for the
placement writer that is explicitly allowed to change that document; and the
thirty-eighth is the release agent trusting an empty ownership record while
an unrecorded proxy from its own interrupted handoff still held the declared
stable bind.
Every instance is
the same thing: **a declaration checked against something narrower than the world.**

The check passes. The declaration is self-consistent. Nothing compares it to
what is actually there.

## The thirty-eight

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
| 19 | `src/cli/mod.rs` and every `mod` declaration in the crate (#212) | the files in the tree are the code that runs | **that anything declares them.** A `.rs` file no `mod` declaration names is not a compile error - it is not compiled at all, and `cargo check`, `cargo clippy`, `git log` and a file listing all look untouched. On 2026-08-31 at 15:48:36Z a commit replaced `src/cli/mod.rs` with a six-day-old copy, deleting nine `pub mod` declarations - `builds`, `database`, `egress`, `fleet`, `product`, `release_evidence`, `release_quarantine`, `service_converge`, `stream` - while all nine files stayed on disk. `main` did stop compiling, but 27 errors away in unrelated callers, and **no diagnostic named a missing declaration**; the whole `cli/fleet/` subtree went dark and the symptom was `dashboard::run` being called with two of its three arguments. A separate `src/queue/secrets.rs` file has been product code compiled into nothing since 2026-07-27. **Now checked:** `stado-rs/scripts/unreachable_modules.py` resolves every declaration from `src/lib.rs` and each `[[bin]]` path and fails on any file the walk never reached - wired into `version-check` before anything is built, with a ratchet file recording the three known ones and their reasons |
| 20 | **our own diagnosis**, and the guard I nearly shipped for it (`configured_object_base_url`) | a `*.ts.net` MagicDNS name resolving to a public address means the name has been hijacked | **that anything checked WHO answered.** On 2026-08-31 `charless-mac-mini.tail6443b3.ts.net` resolved to `208.111.34.11`, `208.111.35.209`, `2607:f740:0:3f::2f0`, `2607:f740:0:3f::3cc` — public addresses, on two workstations, through `getaddrinfo` and not just `host`. Two agents independently concluded that MagicDNS was answering for a stranger and that any caller on that name would leave the tailnet with its bearer token. I wrote the refusal into `configured_object_base_url`, the single gate every origin passes: any `.ts.net` name resolving outside `100.64.0.0/10` / `fd7a:115c:a1e0::/48` is refused. It compiled, and it correctly refused the name with all four addresses listed. Then the one check nobody had made: `openssl s_client` returns **`subject=CN=charless-mac-mini.tail6443b3.ts.net`, issuer Let's Encrypt**, and `whois 208.111.34.11` is **NetActuate, Inc** — the provider Tailscale runs Funnel ingress on. A `*.ts.net` name resolving to a public address is Funnel working exactly as designed: the ingress terminates for that exact name with that node's own certificate, which no stranger can present. **The guard would have refused every Funnel origin in the pipeline** — `STADO_PUBLIC_RELEASE_API_URL` at both publish legs, `deploy-existing-release.yml`, and `version-check.yml`'s `STADO_API_URL` — breaking publication entirely in the name of securing it. Deleted unpushed. `IP is in the tailnet range` is narrower than `the peer is the one named`; TLS already checks the second, and `configured_object_base_url` already requires HTTPS for every non-loopback origin. The first instance here where the suspected wrong answer was a live network peer, and the answer was the right machine all along |
| 21 | `max_items_per_pass`, spent by `build_caches` before any other cleaner ran (#214) | the janitor scanned its budget and reported a healthy pass | **that the budget ever reached the cleaner that mattered.** `build_caches` walks all of `$HOME` and consumed the entire per-pass item budget, so `chromium_clones`, `queue_workdirs` and `backup_twins` were each handed **zero** and reported `scanned 0 eligible 0 deleted 0` — indistinguishable, in the report, from a clean disk. The outcome said `cap_reached`, which is true and names the wrong subject: the cap was reached, by the first cleaner in the list. Raising it does not help — measured by `store-reclaim` at 100,000, the schema maximum, successive passes scanned 46,853 then 50,040 then 67,777 then 88,440 and the twins still got nothing, because the walk is larger than any legal cap. **No configuration reached the end of the cleaner list**, so the janitor was silently doing nothing for the cleaner that mattered while reporting a healthy pass, and the acceptance criterion only passed once `build_caches` was removed from that host **by hand**. #214 gives each cleaner an equal share of what remains, counting only declared cleaners still behind it, rolling unspent budget forward, letting the last take the rest, and making `run_hf` take its share as an argument rather than reading the whole cap. This is the instance that hid longest: every reading of it was true |
| 22 | `service converge`'s verdict for an installed binary (#240) | the version an installed binary reports is the version that was installed | **the installed bytes against the release manifest's digest** - the one fact that separates a delivery from a working tree. At 21:25Z a binary answering `stado 0.13.19` appeared in `~/.stado/bin/stado` on `charless-mac-mini`. That version measured **0 present of 9 on both platforms** and had never been delivered, and `converge` read the situation as a stale DECLARATION - offering, under `--apply`, to write the unverified number into the registry. A version string is whatever `Cargo.toml` said when the binary compiled, so it attests nothing at all. **Now checked:** `service_converge.rs:124` adds `UNATTESTED`, judged before drift against `ATTEST_MATCH` / `ATTEST_DIFFERS` / `ATTEST_ABSENT` (`"staged-match"`, `"staged-differs"`, `"no-staged-copy"`, lines 127-133) by comparing the installed bytes with the staged copy `host release` writes and verifies against the canonical manifest's SHA-256 (`attest_installed`, line 781); `--apply` refuses to offer `declare-version` for a row it cannot attest. Verified live at 22:37:57Z on the real host, where the same command had read `HOST_AHEAD` (`"host-ahead"`, line 100) an hour earlier. It has already caught a second one unaided: `DECLARED 0.13.20 / INSTALLED 0.13.26 / unattested`, no staged copy present |
| 23 | `registry push --force`, `src/cli/registry.rs` (#250) | the operator asked for this document to be published | **that a document was read at all.** The command takes a PATH and falls back to the repository's bundled `data/registry.json` — 65 bytes, `{"schema_version":2,"coordinators":[],"targets":[]}`. On 2026-09-01 the operator directed a two-field change (thresholds 40/42 back to 15/18) and a worker ran `stado registry push --force < /tmp/registry_updated.json`: stdin is never read by this command, so the skeleton was uploaded instead of the correct 38K document sitting in the pipe. The deleted-key guard **refused the first attempt and was right**; `--force` waved the second one through. The canonical registry lost all three targets, the mini's eighteen service declarations, and the `fleets`, `inference`, `placement_profiles`, `release_control` and `service_directory` keys; `stado service reap` then answered that the always-on Mac is not in the canonical registry. Two fixes: a piped body with no PATH is now refused rather than ignored (`-` reads stdin deliberately), and a write that takes a registry from N>0 targets to zero is refused **regardless of `--force`**, behind its own `--allow-empty-fleet` |
| 24 | `store_last_good`, `src/targets.rs` (#250, and independently on `main`) | the last-known-good cache holds a registry worth recovering from | **that the document it is recording still names the fleet.** The gate was `validate_registry`, and an empty `targets` array is schema-valid, so seventeen minutes after the forced push above the product's own recovery path wrote `{"schema_version":2,"coordinators":[],"targets":[]}` into `~/.stado/cache/registry-last-good.json` — 65 bytes, dated after the corruption — and destroyed the one copy it exists to provide. Recovery came from an operator's own snapshot in `~/.stado/work`, not from the cache built for exactly this. Two sessions wrote a guard for it within the hour: an absolute floor (never cache a targetless document) and `may_replace_last_good(incoming, recorded)`, which refuses only `arriving == 0 && held > 0`. **The relative one is what landed**, because the failure being protected against is a document LOSING its contents, and a first-run cache on a machine that has never had a fleet is the honest empty state rather than collateral |
| 25 | `validate_registry`'s rollout rules across versions, `release_control.products.*.targets.*` (#256) | this registry document is valid | **that it is valid for the binaries that must obey it.** `readiness_path` under a `replace` rollout went from **forbidden** to **required** with no version where both hold: 0.13.20 and 0.13.23 answer `replace rollout forbids stable_bind, candidate_ports and readiness_path`, and 0.13.26 and 0.13.27 answer `rollout target requires readiness_path` — measured against the same live document at 06:26Z on 2026-09-01. Validation is whole-document (instance 16), so either shape freezes something: with the key present the mini's 0.13.20 queue agent resolved no policy at all and disk maintenance stopped dead — eight consecutive passes reading `invalid_or_unavailable_policy`, `pressure False`, every cleaner zero, 05:23Z to 05:35Z — and with the key absent **every registry write from the operator's own installed binary was refused**, so a fleet running four versions could only be written by a build older than the one it runs. Fixed additively in #256: a replace target may omit the key and takes `DEFAULT_REPLACE_READINESS_PATH`, blue-green still requires it, and `release_submit` reads the same constant. One document now validates under 0.13.20, 0.13.23 and the patched 0.13.27 |
| 26 | `REAP_SCRIPT`'s keep-set, `src/deploy/service.rs` (#285) | these pids belong to declared labels, so everything else under a managed root is unowned | **one command's printable domain.** The set is built from `launchctl list`, which prints only the domain the calling login can print, so a declared **system LaunchDaemon**'s pid is never in it. On 2026-09-01 `service ensure stado-agent-mini --as-daemon` installed `com.wisent.compute.service.stado-agent-mini` in the system domain and reported `pid: 3963`; ninety seconds later `service reap --command "stado agent"` classified 3963 `would_end` — the tool whose entire purpose is ending *undeclared* processes proposing to kill the one declared agent the host had, with `--apply` one word away. Its argv is byte-identical to the undeclared duplicate beside it (`/Users/charles/.stado/bin/stado agent --target charless-mac-mini`), so no `--command` substring could separate them: the choices were to end the declared agent along with the duplicate, or not to reap at all. The irony is local — `LOADED_LABELS_SCRIPT`, eight hundred lines above it **in the same file**, already carried the comment explaining that `launchctl list` cannot see the system domain. **Now checked:** the keep-set falls back to `launchctl print <domain>/<label>`, which reads that domain unprivileged, and takes only the `pid` line. Keep-set 3 pids → 14, 3963 flipped `would_end` → `kept`, and `--apply` then ended the duplicate and left the declared daemon running |
| 27 | `service converge` / `service show` status, and `com.wisent.host-health-beacon-collect` | this host has gone silent, and this unit's state is unknown | **that the thing reporting silence was itself alive.** Every unit on `charless-mac-mini` read a beacon ~5.9 days stale (508416s) because the collector is not running — `launchctl list` holds it at PID `-`, last exit 1 — so `service list` answered `unit state is unknown` for the whole host and `service show` answered `status='declares'` for the queue agent **while two processes were executing its exact declared program**. Beacon freshness is therefore not an available liveness signal on that host at all, and a reader who takes it for one concludes a working host is dead. `declares` is spelled differently from `runs` in that module on purpose (instance 12's lesson); the trap here is one level out — the *staleness threshold* is the declaration, and nothing compared it against whether its own collector still ran. Liveness had to be established from a live fact instead: `host gates` reporting capacity published 0–53 seconds ago, which no stale artefact can fake. **Not fixed:** the collector is still down, so the signal is still unavailable fleet-wide on that host |
| 28 | the `release` check in `src/doctor.rs`, and `STADO_PUBLIC_RELEASE_API_URL` in both release workflows (#298) | the release channel is reachable | **the route a release actually travels.** The check GETs a 201-byte manifest and passes on any 200, so it passed while `charless-mac-mini.tail6443b3.ts.net` resolved through `1.1.1.1`/`8.8.8.8` — the resolver this machine has for the MagicDNS suffix, which cannot answer a MagicDNS name — to the public `ts.net` front end on one attempt and to nothing on the next, with `100.100.100.100` answering `100.120.25.24` throughout and that address serving the same route in **82 ms**. The 0.13.40 train read one immutable object at 20 MB per 55 s, spent 42 minutes inside `Validate public native release delivery`, and was cancelled at 55 minutes with the object API healthy the whole time. Two narrowings, one name: the check measured the answer instead of the path, and the validation step re-downloaded every object — 300 MB per platform — to prove what the writer read-back had already proven. Instance 20 saw these public addresses and asked whether the name had been hijacked; the answer is that public DNS answers for `ts.net` names by design, and the property nothing held was that a tailnet origin must be reached at its tailnet address |
| 29 | the immutable release coordinate `releases/<product>/<version>/<platform>/`, written by `deploy.yml` and by `cli::release_submit::publish` (#324) | these objects are immutable, so a version means one build | **that anything owned the version.** Create-only puts protect one OBJECT. The two producers write **disjoint** names — the train writes six executables, `SHA256SUMS`, `stado-v<v>-<p>.tar.gz` and `release-manifest-<p>.json`; the signed pipeline writes `release.json`, `release.sig`, `release.tar.gz` and `qualification.json` — so `--if-absent` never refused either of them, and the version number lives in `Cargo.toml`, which many commits share. `stado/0.13.46/darwin-arm64` is the bill: `release.json` attests `446ad490`, `release-manifest-darwin-arm64.json` attests `641a52b2`, both publications succeeded, and `pipeline_catalog_identity` (#266) then refused delivery of a version that means two builds — correctly, and too late, because immutable objects mean it can never be made to mean one. **Now prevented, not merely detected:** `RELEASE_REVISION_NAME` (`source-revision.json`) is claimed create-only, before any artifact, by every publisher through one function — `claim_release_coordinate`, reached from the pipeline directly and from both workflows as `stado release claim-coordinate` — so a second build is refused while the prefix still holds nothing, naming both commits and the one remedy immutability leaves: publish a new version |
| 30 | `/api/object/list`, `ObjectRef::namespace_prefix`, both `authorized_list_prefix` implementations and `StadoObjectBackend::blob_prefix` (#334) | this listing answers for the prefix that was asked for | **the separator that says what a prefix is.** Every one of those layers called `trim_matches('/')`, so `prefix=queue/` became `queue`, and a store scan for `queue` answers with `queue/` **and every sibling whose name begins with those five letters**. Nothing had one until 2026-09-02 at 23:32, when a migration created `queue_priority/`: the next release train read 9026 priority markers as queued jobs, `list_jobs` could not build the terminal-workdir keep-list, and `release-capacity` refused the 0.13.50 train before a single object was published — a disk pass that never looked at a build cache, blocking a release, because of a trailing slash. **Now three things instead of one:** the separator survives from the query string to the store scan; a client filters an over-broad answer instead of refusing a store that holds exactly the right objects, so a fleet still running the old gateway cannot make a new reader wrong; and an unreadable queue costs the one stage that needs it — reported as `SKIPPED`, keeping every workdir — instead of the whole reclamation. The refusal for a genuinely inconsistent item, where `uri`, `namespace` and `key` disagree, is untouched: breadth and integrity were one error message, and they need opposite responses |
| 31 | `fetch_release_object` in `src/deploy/host_release.rs` (#345) | the target can discover how many bytes it must fetch | **who answers the question.** The size came from a `Range: 0-0` request's `Content-Range`, which the tailnet proxy in front of the object API produces and the dashboard's own release route does not. Every delivery worked as long as the target was a DIFFERENT host from the one serving the store; the host that serves it fetched over its own loopback, got no `Content-Range`, and refused with `fetch no_declared_size` — so `charless-mac-mini` could not be given the release it publishes, and `deploy-fleet` failed 0.13.52 four times on a host whose bytes were already public. The operator side knew the number the whole time: `release_object_size` reads it from the channel and `archive_bytes` is bound into the program, so the target is told rather than left to derive, and the range probe survives only as the fallback for a size nobody could read. The same change stopped asking that host for its own public name: the service directory states the address each host uses to reach a loopback service, and `release_origin_allowed` had always permitted it |
| 32 | `cleanup_once`'s `lock_busy` branch at `src/providers/local/disk_cleanup/mod.rs:1938` **and** the `DeployError` -> string -> `classify_message` path through `src/deploy/host_exec.rs:400`, `src/cli/host.rs:2728` and `src/failure.rs:109` | this host's janitor has stalled, and this `host exec` timed out | **the answer that was in hand at the site and thrown away, then guessed back downstream.** Two instances, one property, an hour apart. The janitor: a workload holds the run lock in shared mode for its whole job, so a pass that starts meanwhile answers `lock_busy` — the modelled, healthy answer — and `finish` writes state only when `persist` is `Some`, which that one branch passed as `None`. Line 1353 even special-cased `outcome != "lock_busy"` on a path `lock_busy` could never reach: someone intended to record it and the wiring never carried it there. The janitor is in-process in the agent at a ten-second tick, so roughly **40 prevented passes** left no trace during one 42-minute job on `charless-mac-mini`, `interval_noop` never advanced the stamp, `cleanup_success_age_seconds` reached 2311s against a 1200s limit, and `host gates` turned `claiming` off on a host with **17.3 GiB free, a 15 GiB watermark and `disk_pressure_unresolved: false`** — refusing new work because it was doing work. The classifier: `DeployError` carries no code, `CmdError::click(exc.to_string())` discards it into a string, `classify_message` bare-substring-matches `"timeout"`, and the allowlist refusal **prints the allowlist**, three of whose entries contain `--login-timeout-ms` — so every unapproved command on every host reports `error_code=timeout retryable=true` and every caller retries a decision that will never change. Neither site lacked the information; both dropped it and let an inference downstream reconstruct it wrongly. **Fixed:** the prevented outcome is persisted with its timestamp as `last_prevented_at`, a pass prevented within the stall window is not a stall, and the stall blocks admission only under real pressure. The classifier repair is #343's family and is owned elsewhere |
| 33 | `claim_release_coordinate` in `src/cli/release_cmd.rs` and the claim step inside each publisher of `deploy.yml` (#351, #366) | one coordinate, one build — the remedy written for instance 29 | **the version above the coordinate, and the moment the claim is made.** The record is per `<version>/<platform>`, so each platform was internally consistent while the version was not: `0.14.3/linux-amd64` attests `8cf54ece` with all ten objects published, and `0.14.3/darwin-arm64` attests `ccc43c5e`, claimed minutes earlier by `Submit product release` on an unrelated commit. Each publisher claimed only its own coordinate, and only when it was already holding a built archive — so the Linux leg published everything before the darwin leg learned the version was spent, leaving a half version no attempt can complete. 0.14.2 and 0.14.4 hold exactly one object each, the darwin claim, for the same reason. Two changes close it: the claim now also reads its sibling platforms and refuses a version whose platforms disagree, and the release train claims **every** declared platform in `release-capacity`, before either publisher starts, so a spent version costs one minute instead of one platform's release |
| 34 | `max_pass_seconds` (absent) and `max_scan_items` in `charless-mac-mini`'s registry `disk_cleanup` policy, spent by the cleaner order in `src/providers/local/disk_cleanup/mod.rs:1665` | this host's janitor completed a pass, and its replica holds no reclaimable twin | **whether the last cleaner in the order ever ran.** The policy declared `max_scan_items: 10000` and no `max_pass_seconds`, so every pass took the janitor's own 30-second deadline against a `$HOME` carrying 103.9 GiB under `~/.stado` alone. `build_caches` walks all of `$HOME` by design, and the pass ended inside it: measured 2026-09-03T21:24Z, `backup_twins` reported `scanned 0, eligible 0, deleted 0, skipped {scan_cap: 1, scan_deadline: 1}` while the host sat at 12.8 GiB free against a 15 GiB watermark, refusing every ordinary job for eleven days. `cap_reached` is true and reads like work being done — the same sentence this file records at instance 21, one layer further out: there the budget was spent by the first cleaner, here it is spent by the clock. Declaring `max_pass_seconds: 240` and `max_scan_items: 100000` (generation `2f513ddd`) changed the same pass to `duration_ms 144856`, `deadline: false`, and `backup_twins scanned 66662` — and the answer it then gave is the second half of the finding: `absent_from_primary: 66642`, so those 9.2 GiB at `local-backup/ecosystem/probierz/ecosystem/probierz` are not twins at all but the only copy of instance 3's misaddressed objects, and no cleaner may remove them. Two declarations were narrower than the world here: the pass budget, and the assumption that a replica's contents exist at their primary address |
| 35 | `Cargo.toml`'s `version`, read by `env!("CARGO_PKG_VERSION")` at `src/targets.rs:3148`, `src/providers/local/agent.rs`, `src/providers/local/disk_cleanup/mod.rs` and thirteen other sites | this binary is `0.14.6` | **which tree `0.14.6` means.** #7 said a version number can stand for two materially different trees. On 2026-09-03 it stood for **four**: the binary the fleet was actually running, which carried neither `0abdb82b` (the janitor workload-hold leak fix) nor `df121b88` (builder selection on claimability); `df121b88` itself, whose `Cargo.toml` declares `version = "0.14.6"`; `0abdb82b`, which declares the same; the checkout HEAD `489ba2b7`, carrying all four of the night's fixes; and a local `target/release` build with a fourth combination. No release object existed to disambiguate them — `stado://releases/stado/0.14.6/darwin-arm64/release.tar.gz` measured **absent** while `source-revision.json` measured **present**, a coordinate claimed and never published into, and 0.14.7 and 0.14.8 had neither. So the only way to establish what the control plane carried was **`strings` and `nm` against the installed binary**: `0abdb82b` absent because `disk_cleanup_lock_held` and `cleanup_prevented_age_seconds` were missing from it, `df121b88` absent because its literal `no declared target of that platform is publishing capacity` was missing **and the pre-fix sentence `is broadcasting verified release_platform` was still there**, `1e6448bd` present because the symbol `stado::cli::resolver::Tunnel::usable` existed with zero `alive` symbols left, `02cd36e3` present by its literal. Symbol absence alone proves nothing — inlining can remove a `pub fn` — so every negative needed a string literal beside it, and one candidate literal had to be discarded after it turned out to live in a doc comment at `host_gates.rs:243`. The irony is the point: the commit that opened the window is `8e82eb97`, **"Bind a version to one build across its platforms, before either publisher starts"**. **Now a read:** `58f971de` embeds the source revision at build time and surfaces it in `--version`, in the agent's published `agent_version` and `agent_source_revision`, and in a cleanup report's `writer_version` — `stado 0.14.8 (rev 58f971de6aaf)`, with `-dirty` when the tree had uncommitted changes, and the stated sentinel `unknown` where no git metadata exists, because a tarball build is a legitimate build and a gate that refused it would remove the only route that currently ships fixes |
| 36 | `host recover --release` in `src/deploy/host_recovery_release.rs` against `~/.stado/bin/install-built-stado-binary` | installing the control plane is a gated operation | **that the gated path is the path anything travels.** The product ships an installer that is correct in every detail: it reads three registry-trusted signed objects from a compile-time origin and refuses `STADO_API_URL`, a local resolver and the target's own binary (`:1-7`); it checks the host's identity against the registry and exits 64 on a mismatch (`:285-289`); it compares an `openssl dgst -sha256` against the manifest digest and exits 68 (`:329-331`); it runs `codesign` then `codesign --verify` (`:333-334`); it hardlinks a backup before activating (`:337-341`); it activates atomically (`:346`); it then **smoke-gates the result** — `"$active" resolver --help`, exit 70 on failure (`:349-354`) — and commits only afterwards (`:355`), with an EXIT/HUP/INT/TERM trap that relinks the backup and emits `rollback restored` on any earlier failure (`:303-323`). It is unused, because it needs a published coordinate and the versions in production have none. What actually installs the fleet's control plane is a hand-rolled script with **no signature check, no registry trust, no host identity check, no readiness gate and no automatic rollback**: it prints sha256 rather than comparing it (`:135-136`, `:180`), runs `--version` on the staged file as its only liveness read (`:67-68`), refuses a downgrade (`:137-139`) — its one real guard — copies a dated backup with `shutil.copy2` (`:174-177`, which is why the installed binary's mtime survives a swap and cannot be used to date an install), `os.replace`s the file (`:179`), and leaves recovery as a **printed `cp` line** (`:183`). Ten dated `stado.<version>-backup-<date>` files in `~/.stado/bin` show how routine that route is. Same shape as instance 14, one layer out: there a boundary named "release publication" was enumerated, labelled, described, branched and validated and required by no route; here an installer is signed, digest-checked, smoke-gated and rollback-capable and reached by no caller. The sentence that makes it land: **the coordinate system publishes signed, verified, rollback-capable artifacts for four products, and the binary implementing it is installed by `cp`.** Deliberately still permissive — see "The repair, in order" below |
| 37 | `immutable_job_projection`, durable records under `job-transitions/`, and `autonomy::optimizer::update_job_placement` | a completed transition's saved destination still identifies the queued job it moved | **that placement has its own writer between those two reads.** The autonomy optimizer owns `provider`, `pin_to_provider` and `assigned_to` on a queued document and writes them with a versioned CAS; lifecycle owns the prefix and state. `assigned_to` was already absent from the supposedly complete immutable projection, while `provider` and `pin_to_provider` remained in it. On 2026-09-03 transition `c50f5388…` had correctly requeued `job-cd5dfcf78b6727f30fec0c87` with empty provider placement, then the optimizer selected `local` and pinned it. Every later claim compared the live queue document with the old destination, reported `does not match completed transition`, killed the whole agent tick, and left seven unrelated pinned jobs waiting for eleven days while capacity and health continued to publish. The fix gives each field one owner: the durable request digest still protects caller-supplied placement options, the shared immutable stored-job projection excludes all three optimizer-owned placement fields, and lifecycle continues to verify every other identity field plus the destination state. A completed generation retires itself as `retired:destination-verified-source-retired` through a versioned CAS rather than an unsafe path delete, an unknown label such as the observed `done` is settled from the actual source and destination, and one job's remaining claim error is published with that job id while the scan continues to the next job. |
| 38 | `src/release_agent.rs`'s proxy ownership record and `deploy/recover_object_api.sh`'s startup health snapshot | the release state has no proxy pid, the legacy launchd service owns the stable bind, and the object boundary was ready at startup | **that a standalone `stado release proxy` from an interrupted agent handoff still held the exact product state path and stable bind.** The process sweep enumerated only binaries under the product's release directory, so it could not see a proxy launched from Stado's own managed binary; the quarantined rollout then returned before reconciling the bind. On 2026-09-04 Skarbiec's declared legacy unit looped with `Address already in use` while the surviving release proxy forwarded `127.0.0.1:8895` to a dead candidate on `127.0.0.1:18895`, and the object API returned 503 because every protected request revalidated through that dead boundary. `recover_object_api.sh` answered `already_healthy` from cached `/healthz` `"object": true`, measuring the startup declaration rather than an authenticated object read. A third narrow declaration kept both repair readers from restoring the service: `release_control.products.skarbiec.targets.charless-mac-mini` carried null `legacy_launchd_label` and `legacy_launchd_plist`, while the same host's canonical `services` entry named `skarbiec` and declared the exact launchd label and plist. Rust and the bootstrap now resolve only that one service whose name equals `ProductReleasePolicy.service`, require an exact launchd kind and canonical system label/path pair, and refuse partial explicit fields or ambiguous declarations; complete explicit release-target fields remain the compatibility override. The release agent is the durable owner: under its per-product lock it matches the proxy's exact executable, `--state` and `--bind`, adopts and persists its pid only when a managed upstream is ready, or refuses ambiguity and waits for the sole exact orphan pid to exit before restoring and verifying the declared legacy service. The checked-in recovery helper is the bootstrap for an older installed agent that cannot reach its own replacement: before the protected object read, it derives the Skarbiec state, proxy, candidate, bind, readiness, label and plist from the host's last-good registry; requires empty active/candidate/previous/proxy ownership, a dead or unready declared candidate upstream, one exact executable argv, that pid actually listening on the stable bind, and a matching plist label; proves every restoration prerequisite before TERM; then waits for only that pid to exit, idempotently bootstraps the declared unit and requires both launchd presence and stable readiness. Both early and final object recovery success require the authenticated protected read itself. |

The observed `c50f5388…` record had already been labelled `aborted`, even
though its source was `transition-cleaned:c50f5388…` and its queued
destination matched once optimizer-owned placement was excluded. Recovery now
checks both sides of an aborted record too: only a matching destination with
an absent, fenced, or generation-matched cleaned source is completed and
retired, so the stored label cannot preserve an earlier wrong reading.

## The property they share

In each case the system stored an *intent* and then re-read its own intent as
evidence. A name assumed to lack a prefix. A watermark read from one place and
enforced from another. A key assumed to be bare. Replication assumed off
because config said off. A publish assumed complete because the loop exited. A
tag assumed to mean bytes exist. A timeout assumed to be enough. A port
assumed to have one holder, counted with a tool that could not show two. A
boundary assumed to gate what its name says. A binary assumed to be current. A
state file assumed to have one writer. A file on disk assumed to be code. A
name assumed to be hijacked because its address was public. A budget assumed
to reach the cleaner it was declared for. A binary assumed to have been
delivered because it reports a version. A keep-set assumed to hold every
declared label's pid. A host assumed silent because the thing that reports on
it stopped reporting. A version assumed to name the code that carries it. An
installer assumed to be on the path because it exists and is correct.

Several of these passed a validator, a schema or a health check first, and one
of them passed a fix aimed at that very defect. So: **a defect that survives a
check tells you the check models the wrong thing.**

And the last two say where to look next. **A keep-set, an enumeration and a
staleness threshold are declarations too, and when the narrowing is in the
instrument, nothing downstream can notice.** Every other instance here was
caught because some reader eventually compared a claim against the world. An
instrument has no such reader: its output *is* the world as far as everything
above it is concerned, so a window that is too small produces answers that are
internally consistent, confidently wrong, and unfalsifiable from the outside.
Instance 26 was found only because a second tool disagreed with the first, and
instance 27 only because a live fact was available to replace the dead one.
That is why the rule below is to check the instrument, and why "our tool said
so" is the start of an investigation rather than the end of one.

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
8. **When a guard refuses, the answer is never the same command with the guard
   disabled.** On 2026-09-01 the operator directed a two-field threshold
   change, a worker's `registry push` was refused by the deleted-top-level-key
   guard — correctly, because the document it had built was not the one
   intended — and the worker re-ran the identical push with `--force`. The
   canonical registry lost every target, every service declaration on the
   always-on Mac, and five top-level keys; seventeen minutes later the
   last-known-good cache recorded the wreckage as good. The guard was the
   product working. A refusal is a measurement of the input, so the response
   is to fix the input or to learn what the guard knows — never to re-issue
   the write with the measurement switched off. This is the same lesson as
   deleting a CI check to get past it, one layer down, and it is why the two
   fixes for instance 22 are a refusal that `--force` **cannot** cross and a
   separate flag that names the intent out loud.
9. **A gate nothing reaches is not a gate.** Instance 14 was a boundary that
   was enumerated, labelled, described, branched and validated and required
   by no route. Instance 36 is an installer that is signature-verified,
   digest-checked, identity-checked, smoke-gated and rollback-capable and is
   reached by no caller, while a script with none of those properties installs
   the fleet's control plane. Reviewing a gate's contents cannot find this,
   because its contents are correct; the only question that finds it is **who
   calls this, and what do they use instead.** Ask it of every gate whose
   precondition is something the fleet does not currently produce.
10. **A version identifies a release, never a build.** A version string is
   whatever `Cargo.toml` said when the compiler ran, so several trees can and
   do carry the same one — four of them, in instance 35. Anything that has to
   answer "which code is running" must carry the revision, and anything that
   has to answer "which is newer" must not: `minimum_stado_version`, the
   agent's release-handoff comparison and `self_update` order versions, and a
   revision has no order. Carry both, separately, and let the ordered one stay
   a bare semantic version.

## The repair, in order

Instance 36 is the one finding in this document whose fix must **not** be
applied first. The order matters more than any single step, and each step is
the precondition of the next.

1. **Publish real coordinates for the `stado` binary.** Nothing downstream is
   possible without this. `host recover --release` verifies three signed
   objects, and for every version the fleet actually runs there are none:
   `0.14.6` has a `source-revision.json` claim and no archive, `0.14.7` and
   `0.14.8` have neither, and the newest published stado coordinate is
   `0.14.4` — the one version nobody is running. Until a coordinate exists,
   the gated installer has nothing to install.
2. **Move installation onto the gated path.** Install with
   `host recover --release <VERSION>`, and let `host declare-version` and
   `host reconcile` state and converge the desired version, so what runs on a
   host is always bytes that were signed, digest-checked and smoke-gated with
   a rollback behind them. This cannot come first: it presupposes step 1.
3. **Only then close the bypass.** Retire `build-stado-binary` and
   `install-built-stado-binary`, or make them refuse bytes that do not match a
   published coordinate.

**Why closing the bypass first would be the worst available choice**, and this
is the whole reason the order is written down: the ungated script is currently
the *only* route by which a fix reaches a host. Making it refuse while no
published coordinate exists would leave this fleet unable to ship at all —
including unable to ship the change that closes the gap. That is rule 8 read
forwards: a refusal is a measurement, and a gate whose precondition is the
outcome it is meant to enable is instance 15's shape, already in this table.

What instance 35's fix buys for every step above is the ability to check them.
Each one needs to know what a host is actually running, and until `58f971de`
that meant `strings` and `nm`. It is now `stado --version`, `agent_version`
and `agent_source_revision` in the capacity diag, and `writer_version` on any
cleanup report. Step 3 in particular becomes auditable rather than assumed: a
binary installed by the bypass out of an uncommitted tree now says `-dirty`
about itself.

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
- **Nobody can name what wrote a binary onto the always-on Mac at 21:25Z.** The
  fleet ruled out the release channel by measurement (`0.13.19` was 0 of 9 on
  both platforms), `host release` under any invocation (its dry run had the host
  at 0.13.13 minutes earlier, and it verifies the archive against the manifest
  before staging), this repository's automation (`deploy.yml` delivers only
  after a publish that never happened), and both agent sessions - and the writer
  is still unnamed. `store-reclaim` is adding a delivery receipt beside the
  staged copy - version, manifest digest, actor, timestamp - so the next such
  binary answers the question instead of raising it. Until that lands, instance
  22 detects the condition and cannot attribute it, which is a smaller gap than
  before and still a gap.
- **`stado-v0.13.19`'s train was cancelled at 21:59:51Z inside step 3,
  `Build native Rust control plane`,** on a per-ref concurrency group
  (`wisent-compute-release-train-${{ github.ref }}`) with both product-release
  groups set to `cancel-in-progress: false` - so it joins `0.13.7` as
  unexplained, and per-tag concurrency has not absorbed it either; the
  coordinate measured 0 of 9 on both platforms, so archive-first ordering is the
  only reason it cost nothing.
- **`0.13.18` finished 9 of 9 on both platforms after being caught mid-loop** at
  4 objects and then 5 in two consecutive readings, which is why a rising
  coordinate must never be reported as a partial: two readings that disagree
  mean the write is still happening.
- **The four permanent partials above stay on the list.** They are not
  historical trivia; they are the reason every check in this document exists.
- **Nothing in Stado can verify or repair a build host's cargo registry, and
  the janitor was entitled to delete it.** `~/.cargo/registry` carries a
  `CACHEDIR.TAG` byte-identical to the signature `build_caches` matches, so on
  `lukasz-macbook` - the single runner that publishes every release, in
  `enforce` mode with the cleaner's root defaulting to `$HOME` - the registry
  was an eviction candidate. Measured with a seeded home: before the fix the
  cleaner reported the registry and a project `target/` as **2 eligible items,
  16,392,192 bytes**; after reserving it, **1 eligible, 8,196,096 bytes**, with
  the registry counted under `reserved_or_hidden` and `target/` still eligible.
  Everything else this cleaner removes is output that the next build
  reproduces; the registry is input, shared by every build, and it returns only
  by re-fetching from a network that has to answer.

  **On the cause of `0.13.14`'s build failure, corrected.** That step died with
  `aws-lc-sys` reporting `no such file or directory` for two vendored C files
  inside this registry, and the extraction verified complete afterwards - all
  2010 files present with matching sizes against the `.crate` archive - so
  nothing was cleared. It was first recorded here as transient. The likelier
  reading, and the one the operator named, is that the janitor armed on that
  runner deleted registry files under a running build and cargo restored them
  afterwards: an eviction followed by a re-fetch is indistinguishable from a
  transient once both have finished. The causal chain runs back to arming
  `enforce` on a build host without first checking what the cleaner's signature
  actually matched - `~/.cargo/registry` carries cargo's own `CACHEDIR.TAG`,
  and nobody read it before declaring the policy. "Transient" was the
  comfortable reading of an absence of evidence.

  The gap that remains: the integrity check that established the extraction was
  intact - comparing all 2010 files against the `.crate` archive - was written
  by hand for one crate. No `stado` command does it, and `build_caches` covers
  target trees, not registries.
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

**#7 stopped being hypothetical on 2026-09-01: `0.13.27` is attested from two
different revisions, and both attestations are signed and published.** The
repository has two release paths that write into the same version coordinate
and neither reads the other's provenance:

| Path | Object | Field | Revision |
|---|---|---|---|
| product pipeline (`stado release submit` -> `publish_pipeline_release`) | `stado://releases/stado/0.13.27/darwin-arm64/release.json` | `source_revision` | `d53f10c9228befba188a8b780dce68a041929c50` |
| tag deploy (`deploy.yml`, `stado-v0.13.27`) | `stado://releases/stado/0.13.27/darwin-arm64/release-manifest-darwin-arm64.json` | `source_commit` | `99e033960d1aa7b426c427027bb445f658b3215b` |

How it was measured, so the next reader can repeat it rather than trust it:
`stado storage get` each object and read the field. `git rev-list --count
d53f10c9..99e03396` is **14**, and those fourteen carry #250, #251, #255, #256,
#257 and #258. Both coordinates are complete: the pipeline objects are 4 of 4 and the
deploy objects are 9 of 9, on **both** platforms. Neither path is broken and
neither is lying; they simply answer the same question, "what source is
`stado` 0.13.27", with different commits.

The consequence is concrete rather than theoretical. `install-stado.sh`,
`self_update.rs` and `local_install.rs` all resolve
`release-manifest-<platform>.json`, so an installed binary traces to
`99e03396`. `host_release.rs` and `stado release status` resolve `release.json`,
so the fleet's own rollout provenance traces to `d53f10c9`. At three in the
morning, "which source is this host running" has two correct and different
answers depending on which command is asked, and nothing anywhere compares
them.

**The unprotected property, named:** no gate asserted that every object
published under one `product/version` coordinate attests the same
`source_revision`. The immutability rule protects each object from being
rewritten; it does not stop two paths from writing mutually inconsistent
provenance into one coordinate.

**The delivery half of that is now closed (#266).**
`pipeline_catalog_identity` in `src/deploy/host_release.rs` reads
`release-manifest-<platform>.json` beside the signed manifest and refuses when
their revisions disagree, naming both, with the remedy `catalog_identity`
already states thirty lines below it for an incomplete coordinate: immutable
objects mean the version can never be made to mean one build, so publish a new
version. It is a refusal `--force` cannot cross, and a coordinate published by
one train is unaffected — the sidecar carries that train's `GITHUB_SHA` and the
signed manifest the same revision, or there is no signed manifest and the
legacy path already resolves the train's own archive. This was found by
delivering `0.13.27` to `charless-mac-mini` and watching `host release` report
`released: charless-mac-mini now runs stado 0.13.27` while staging the
`d53f10c9` archive, with `service converge` then confirming `in-sync` — every
reading true about itself and none of them true about the version that was
asked for.

**The writing half is now closed too (#324), including its cross-platform
race.** A platform-scoped create-only claim still permitted two publishers to
observe no sibling and atomically claim opposite platforms from different
commits. Every publisher now first arbitrates the platformless
`releases/<product>/<version>/source-revision.json` through the same
`claim_release_coordinate` implementation, then writes the compatibility claim
under its platform. A create-only write at version scope has one winner no
matter which platform order or platform set a publisher carries. Backfilling
an older coordinate requires an exact version-prefix inventory in which every
existing platform has a valid, coherent claim for the same commit.

Readers do not merely notice the new object. The release-channel inventory
represents a version claim that crashed before any platform write, excludes the
version record from platform member sets, and compares it to platform claims
and manifests. Promotion and delivery bind the artifact source to that shared
claim, with a validated platform-claim fallback for coherent releases that
predate the version record.

**#7's worst reading arrived on 2026-09-03, and the half that hurt is now
closed (instance 35).** Everything above concerns two paths disagreeing about
one *published* coordinate. The sharper case needs no publisher at all: a
version with **no** release object, carried by a binary installed outside the
coordinate system entirely. `0.14.6` named four different trees — one of them
the binary the fleet was running, missing two of the night's fixes — and with
no artifact to read, provenance had to be recovered with `strings` and `nm`.
`58f971de` closes that by embedding the source revision at build time and
surfacing it wherever the version is surfaced, so the question is answerable
on any host from the binary itself, with no coordinate, no store and no
network. `VERSION` deliberately stays a bare semantic version, because
ordering depends on it.

What that leaves of #7 is its original form and no more: autoversion can still
let one version number stand for two trees, and the release rule that would
fix it is still a deliberate decision rather than an incident change. The
difference is that a wrong answer is now detectable from the binary instead of
being unfalsifiable — which is the rule this document keeps arriving at, that
an instrument's output *is* the world as far as everything above it is
concerned.

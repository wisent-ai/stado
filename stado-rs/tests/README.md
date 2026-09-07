# What a complete Stado test suite looks like

Written 2026-09-06 and checked against `main` at `a2d62b4f`. Every count below
was read from that revision, not remembered.

This file is the plan for the test tree it sits in: the rules each test obeys,
what the tree measurably covers today, and the coverage that is missing. It is
not a second definition of done — that lives in `~/STANDARD.md` and the stage
gates in `product-guidelines/README.md`. The rules restated here are the
operator's, set 2026-08-19 in `~/AGENTS.md`.

## The rules every test obeys

**One binary, driven from outside.** A test runs `CARGO_BIN_EXE_stado` through
`std::process::Command`. Not a library function, not a mock, not a re-implemented
code path. A dry run, a smoke test, a canned response, a simulated provider, a
schema check and a successful exit before the promised effect are not tests and
never count as evidence that a feature works.

**Assertions read state, not output.** The document on disk, the object in the
store, the file on the host, the exit code, the exact refusal sentence. Stdout is
supporting evidence, never the only one.

**Every command is a set of paths.** Success plus its refusals — duplicate,
malformed name, unknown object, occupied state — each asserted with its exact
sentence, because that sentence is part of the product contract.

**One test target is one command and its story.** `create` with its refusals,
`assign` with its refusals, `delete` with its refusals. The test name says what
it defends.

**Isolation through the environment.** A tempdir plus the product's own
backend-overriding variables. `tests/fleet/main.rs` is the reference
implementation:

```rust
cmd.env("WC_STORAGE_BACKEND", "local")
   .env("WC_LOCAL_STORAGE_PATH", storage)
   // A set-but-missing STADO_CONFIG disables config-file discovery.
   .env("STADO_CONFIG", storage.join("no-such-config.json"))
   .env_remove("COMPUTE_API_KEY")
   .env_remove("COMPUTE_API_URL")
   .env_remove("WC_PROFILES_DIR");
```

No test touches the operator's real registry, vault or configuration. Commands
that write to real operator state — invite minting, for one — are out of scope
until a dedicated fixture exists.

**Behaviour is probed before it is asserted.** Exit codes and refusal sentences
are copied from live answers, never guessed. If a command does not exist, the
command is implemented first and the test second.

**A real dependency is used or the run is blocked.** When the feature needs a
host, a provider, a browser or a persisted credential, the test uses that real
component through Stado and Weles and observes the final state. When the real
flow cannot run, the answer is `blocked` with the reason — never an easier test
and never `passed`.

**Retention.** Each run records the exact source revision, the commands, the
exit statuses, the process output and the supported reports, so the answer to
"which revision passed this" is a fact and not a memory.

**Tests are not run without an explicit instruction.** Verification of a change
is `cargo test --no-run` plus manual probes of the product commands.

## What the tree measures today

Read from `a2d62b4f`:

- 64 directories and two loose files under `stado-rs/tests`. 62 of the
  directories carry a `main.rs`, and cargo auto-discovers `tests/<dir>/main.rs`,
  so those 62 are real test targets.
- `platform-matrix/` and `support/` carry no `main.rs`, so neither is a target.
- `stado-rs/Cargo.toml` declares 6 `[[test]]` entries — `channel`, `host_exec`,
  `apple_challenge`, `service_convergence`, `documentation`, `runner`. Auto
  discovery already finds all six; the declarations are redundant.
- Two loose files: `release_quality_gate.sh`, the gate a person runs and
  `quality_gate/main.rs` drives, and `probierz-rust-journey.mjs`.
- 46 of the 62 spawn `CARGO_BIN_EXE_stado`. The other 16 never do:
  `agent_janitor`, `boundary`, `build_registry_skew`, `builder_claimability`,
  `cleanup_gate_naming`, `disk_scope`, `documentation`, `janitor_keep_list`,
  `janitor_refusal`, `object_auth_verdict`, `quality_gate`,
  `registry_cache_refusal`, `runner`, `seed_freshness`, `truncation`,
  `workload_hold`. Two of those are legitimate — `quality_gate` drives the
  shell gate and `documentation` reads documentation sources — and the rest
  need reading against the rule above. `runner/` is mine, written 2026-09-06:
  it asserts the text of the installer in `deploy/host_precheck_runner.rs` and
  runs the job gate extracted from it against a scratch directory. It never
  registers a runner on a host and never reads a host's state, which is the
  same defect as testing a documentation linter instead of the product.

## The tree is not addressable by command

The 62 areas are named after the defect each was written for — `silence`,
`truncation`, `workload_hold`, `stale_unit_image`, `cleanup_gate_naming`. That
is a good name for a regression and a useless one for the question an operator
asks: **is `stado storage verify` tested?**

So the index below exists, and it is a name-level measurement, not a claim
about behaviour: an area is counted for a command group when its test source
names that group as a string literal and the area spawns the product binary.
It answers "could this area possibly touch that command", which is the weakest
useful question, and it is still enough to find the holes.

**Command groups no area names at all** — 20 of the 49 the binary lists:
`onboarding`, `blast-radius`, `resources`, `optimize`, `billing`, `azure`,
`cloudflare`, `mail`, `machine`, `quota`, `profiles`, `schedule`, `cost`,
`vast`, `instances`, `alerts`, `placement`, `database`, `web`, `stream`.
`machine` is the documented machine interface, so the surface a caller
integrates against has no test of its own; `billing`, `azure`, `cloudflare`
and `vast` are provider paths that spend money.

**Groups some area names**, with how many areas name each: `host` 23,
`service` 19, `storage` 19, `status` 14, `registry` 8, `agent` 8,
`dashboard` 7, `capabilities` 7, `release` 5, `product` 5, `doctor` 5,
`resolver` 4, `submit` 4, `config` 3, `coordinator` 2, `cancel` 2,
`queue` 2, `identity` 2, `disk-cleanup` 2, `builds` 1, `fleet` 1,
`results` 1, `job` 1, `artifact` 1, `egress` 1, `overview` 1, `recovery` 1,
`bootstrap` 1, `credentials` 1, `dns` 1, `inference` 1.

A count above one is not coverage: `queue` is named twice, by `ci-cd` and
`run_history`, and neither is a test of the queue's own story. That is what the
next section is for.

## Areas to add, in build order

Each row is a command whose own story nothing tells today, even where the word
appears inside another area. Ordered by what breaks the operator soonest, not
by what is easiest.

| `tests/<area>/` | Drives | Asserts against |
|---|---|---|
| `host_disk` | `host disk --json` | free kibibytes and the swap line for a host under real memory pressure; the inventory roots per platform; that a host that cannot be asked reports so rather than `null` |
| `host_reclaim` | `host reclaim --dry-run`, `--apply` | bytes actually freed per stage, `runner_work_trees` among them; the audit record on the host; the refusal when no stage is eligible |
| `host_grant` | `host grant-item-read`, `grant-show` | the grant recorded in Skarbiec, read back per item and per field; refusals for absent item, absent field, absent bearer file |
| `host_beacon` | `host publish-beacon FILE [--print]` | the beacon object in the store, its reported-at stamp, and `host link` turning stale into `silent` |
| `queue_lifecycle` | `submit`, `status`, `results`, `cancel` | the object moving `queue/` → `running/` → `completed/` → `uploaded/`; `cancelled/` written on cancel; the terminal outcome retained in run history |
| `queue_drain` | `queue drain --wait`, `pause`, `resume` | the paused control blob; the timeout exit being non-zero with the queue still paused; that `CONFIRM_FLEET_DRAINED` alone drains nothing |
| `storage_move` | `storage copy`, `verify`, `stat`, `ls`, `cat` between two local backends | both stores compared object for object; metadata keys folded as the copier folds them; `absent` and `unreachable` (`infra_down`) answered differently |
| `registry_write` | `registry host add`, `remove`, `registry doctor`, `registry self` | the registry document and its generation fence; the refusals for unknown target kind, a target with no SSH destination, a malformed `weles` block, a non-string `host_heuristic` |
| `machine_interface` | `machine submit --request-file`, `status`, `logs --cursor`, `cancel`, `artifacts` | the reserved request under `machine_requests/<id>.json` with its SHA-256; artifacts written to `--output-dir`; the refusal for a field outside the accepted list |
| `release_publish` | `release submit`, `promote`, `quarantine list/clear`, `redeliver`, `active-binary` | the published manifest read back from the channel and byte-compared; the refusals `release job returned mixed or invalid output`, `release job omitted archive`, `immutable queue object differs`, `refusing a replacement without terminal failure` |
| `service_lifecycle` | `service ensure`, `converge`, `restart`, `list` | the unit loaded on a real host and the bind held afterwards; an in-place restart leaving no window with nothing running; the reload refusal naming the program it compared |
| `doctor` | `doctor` | each check's own verdict, and that two checks never disagree about the same backend |
| `desktop_parity` | the Desktop screens through the Probierz CUA harness | each screen showing what the equivalent command answers, on the same state |
| `runner_registration` | `host precheck-runner install --repository`, `status`, `remove`, `restart` | the registration record under `<runner root>/.stado/registered-runner` on a real host, the scope GitHub actually accepted, the listener's launchd or systemd state, and the job slot the gate holds. This replaces `runner/`, which asserts installer text instead |

## The groups that spend money

`billing`, `azure`, `cloudflare`, `vast` and `instances` reach a provider that
charges. A test that provisions there is a purchase, so it does not get written
on an agent's judgement: the cost is stated to the operator first and the run
stays `blocked` until that decision exists. What can be tested without spending
is the refusal side — a missing credential, an unentitled platform, a quota
already at its ceiling — and that is where these areas start.

GCP is not on that list on purpose. Billing is detached from
`wisent-480400` deliberately, so a test that needs a billable GCP API is not a
test to enable; the dependency is the thing to remove.

## What this plan is not

It is not a promise that a folder per row makes the product tested. A row earns
its place only when a plausible bug fails it. A test that pins wording,
plumbing, a field copy or a default asserts an implementation, and belongs
deleted rather than re-pinned.

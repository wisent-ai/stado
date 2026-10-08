# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

### Fixed

- **A job that stages nothing on the disk no longer holds the janitor's lock (91cef9bb):** every claimed job took the shared cleanup hold, including an in-place Oko routine that clones nothing and writes no output. One such routine (the transcript-ledger sweep) hung asleep on a socket, and its hold refused every janitor pass for eight hours (`lock_busy_workloads`, `held in shared mode by 1 running workload(s): job-3dbb5d87`) while the laptop sat at 90% with a 225 GB cargo `target/` tree and 55 GB of Hugging Face blobs the pass deletes. A job that stages nothing (no repository, packages, pre-command or output mirror) now runs without the hold: nothing a cleaner takes is its, so its hang cannot keep a pass from running.

- **A published SwiftPM input restores anywhere:** SwiftPM's checkouts borrow their objects from the mirrors under `repositories/` through `.git/objects/info/alternates`, which names each mirror by the publisher's absolute stage path, so every unpacked checkout pointed at a directory that no longer existed and Git refused it (`unable to normalize alternate object path: …/release-input/swiftpm-…/.build/repositories/echo-…/objects`, b6996b35). `pin-input --swiftpm` now rewrites each alternate relative to its checkout before packing, restoring the file's read-only mode, and refuses an alternate outside the stage. An interrupted publication's gigabyte-sized stage and archive are named `swiftpm-<pid>-…` and removed by the next publication once that process is gone. `tests/release/swiftpm-input.mjs` passed against most-desktop: publication, the sweep (a dead process's stage removed, a live one's kept), restore and every pinned checkout at its revision.

### Removed

- `stado host disk-cleanup TARGET` (3f84dea2). Running a host's janitor from
  elsewhere is `stado space reclaim TARGET --stage registry_cleanup`: it
  previews by default, `--apply` requires `--reason` and is audited on the
  target, and the report carries the free space before and after. `stado
  disk-cleanup` stays the verb for the machine it is typed on.
- `stado release policy-apply`, `policy-remove` and `policy-target-remove`
  (89430d1f): the rollout policy is one command group, `stado release policy
  apply|show|list|remove|remove-target`. `show` prints `{product, policy}` in
  the shape `apply --file` reads and `list` names every release-controlled
  product with its targets and desired release; neither existed before.
- `deploy/reports/` (be05117e): thirteen diagnostic shell scripts (build,
  capacity, network, release, service) that no Stado command ran, no workflow
  called and no page documented. Each was a one-off program kept beside the
  product; one probed guessed loopback ports 8000-8003 and 8080 instead of the
  declared inference services, another cut its output at 140 characters and
  five lines. What they read is answered by product commands: `host
  inventory`, `service show`, `service unit show|logs`, `service env check`,
  `service serving`, `release logs` and `space report`.

### Changed

- **`stado release active binary`, `stado release local install|restore` and
  `stado service image refresh` (6948c848):** the release this host runs, a
  release archive on this host and a unit's executable image each become an
  object with its verbs. `release active-binary`, `release install-local`,
  `release restore-local` and `service refresh-image` still parse, hidden from
  help, because Weles, the host-exec allowlist, the adopt handoff, every
  product's delivery argv and the `service update` host script run them with a
  host's installed Stado, which may predate the groups. Those callers move,
  and the old names go, once a release carrying the groups is on every host.
  The operator console treats `release active binary` as a read.

- **`stado service unit logs LABEL --host H --lines N` replaces `stado host
  unit-log` (24535a86):** the tail of a unit named by its label now sits with
  `service unit show`, the other read of that object. The release delivery
  refusal for unbound stable binds named `host unit-log` and printed a run of
  spaces in the middle of its sentence; the fleet-shape restart remedy named it
  without the `--lines` it requires. Both name `service unit logs` with
  `--lines`. The operator console treats `unit logs` as a read.

- **`stado credentials sparkle-key mint` and `stado credentials
  signing-profile ensure` (e56caec5):** `credentials sparkle-key PRODUCT` and
  `credentials item signing-profile` wrote keys and items under a noun with no
  verb. Flags and output are unchanged. Stado Desktop's credential operations
  run `signing-profile ensure` and gain **Mint a desktop product's Sparkle
  update key**.

- **`stado release coordinate claim` and `stado release staged activate`
  (58b4259c)** replace `release claim-coordinate` and `release
  activate-staged`; flags and output are unchanged. Stado Desktop's
  **Activate a verified staged release** marked product, env file and port
  optional although the command requires all three, so leaving one blank
  ended in a usage error; the form now requires them.

- **`stado service directory consumer add|remove` (d6242149)** replace
  `consumer-add` and `consumer-rm`; flags and output are unchanged. The
  operator console treated every `service directory` command as a read, so
  `consumer-add`, `consumer-rm` and `publish --prune` wrote the registry or
  deleted forward markers without asking; now only `directory show`,
  `profiles`, `bind`, `connect` and `endpoint` are reads. Stado Desktop's
  Routes operations run the new verbs.

- **`stado service watch`, `unit show`, `onboarding set|catalog`,
  `runner-runtime repair` and `handoff` (654eb105):** `service watch-spawn`,
  `label-print`, `onboarding`, `onboarding-catalog`, `repair-runner-runtime`
  and `handoff-release-control` named their objects in the verb, and
  `service onboarding NAME` wrote the registry with no verb at all. Flags,
  output and exit statuses are unchanged. The fleet-shape remedies for a
  doubled label prefix name `stado service unit show <label> --host <host>`;
  Stado Desktop's **Repair GitHub runner runtime** runs `service
  runner-runtime repair`; the operator console treats `watch`, `unit show`
  and `onboarding catalog` as reads.

- **`stado service grant show|mint|sync`, `token-file sync`, `auth check`,
  `secret sync` and `file sync|fetch` (24848d01):** `service grants` (and
  `grants --apply`), `grant-sync`, `token-file-sync`, `auth-check`,
  `secret-sync`, `file-sync` and `file-fetch` spelled their objects into the
  verb. `grant show NAME` prints the declared grants and mints nothing,
  `grant mint NAME [--vault-file] [--ttl-seconds]` mints them (the former
  `--apply`), and `grant sync` is the former `grant-sync`; the others keep
  every flag. The operator console ran `auth-check` as a read although
  `--repair` synchronizes the secret and restarts the unit; `auth check` is
  now a read only without `--repair`, and `grant show` is a read.

- **`stado service env show|set|unset|check` (b2a8e649):** one unit's
  environment was five verbs: `env`, `env-show`, `env-set`, `env-unset` and
  `endpoint-check`. `env show NAME [--host H]` reads the environment the unit
  file declares, as `env` did; `env show NAME --host H --env-file F [--reveal
  KEY]` reads the sourced env file line by line, as `env-show` did (`--env-file`
  without `--host` is refused with `service env show --env-file reads one
  host's file; name the host with --host`). `env set`, `env unset` and `env
  check` are the former `env-set`, `env-unset` and `endpoint-check`; output and
  exit statuses are unchanged. The fleet-shape remedy for a unit whose program
  reads a variable the plist does not hand it named `stado service env-set
  <label> <KEY> <value>`, which no Stado parsed; it names `env set` with
  `--key`, `--env-file` and `--value-file`. The operator console treats `env
  show` and `env check` as reads.

- **`stado cloud roles repair` (ff3ef00d)** replaces `stado cloud
  repair-rbac`. `cloud login --role ROLE` and `cloud roles repair
  --operator-role ROLE` no longer assume the vault role
  `stado-azure-operator`: the operator names the role the session is stored
  under and read from, and clap refuses a command that does not.

- **`stado quota request create|list` and `stado quota ticket reply|escalate`
  (1b82fafd):** `quota request ACCEL`, `quota request-all`, `quota requests`,
  `quota replies` and `quota escalate` spread two objects over five verbs.
  `request create ACCEL --to N` asks for one accelerator and `request create
  --every-family --to N` for every family the catalog reports (exactly one of
  the two is required); `request list` is the former `requests`; `ticket
  reply|escalate --provider azure [--dry-run]` are the former `replies` and
  `escalate`. `--justification` is now required: the two compiled sentences
  every request sent a provider's reviewer when the operator wrote none are
  gone. `stado doctor`'s quota remedy named `quota request --accel <ACCEL>
  --new-limit <N>`, flags no Stado had; it names `quota request create <ACCEL>
  --to <N> --justification <TEXT>`. The MCP tool `stado_quota_requests` runs
  `request list`. The operator console ran `quota replies` as a read although
  it posts to Azure support; `ticket reply|escalate` now ask for confirmation
  unless `--dry-run` is given. Stado Desktop's provider operations gain the
  catalog, request list, request create and both ticket actions.

- **`stado resources apply` executes either reviewed plan; `kill-irrational`
  is gone (3c3fb384):** `resources apply` executed shutdown plans and
  `resources kill-irrational` executed rationalization plans, though every
  plan already records its intent. `apply --plan P --expect-hash H` now runs
  the plan as its intent says: a rationalization plan previews without
  `--yes` and takes `--approve <action>` and `--allow-irreversible` as
  `kill-irrational` did; a shutdown plan needs `--yes` and refuses those two
  flags (`a shutdown plan runs every action it holds; --approve and
  --allow-irreversible select among a rationalization plan's actions`). Any
  other intent is refused with the two plan commands named.
- **`stado resolver api reassign --target H` (3c3fb384)** replaces `resolver
  api-reassign`; behaviour and output are unchanged.

- **`stado service converge` is gone (dea9fe6a):** it was a second command
  for what `stado release version show|converge` does, calling the same
  implementation, and its help still listed a `drifted` verdict and delivery
  through `stado host release`, neither of which exists. Use `stado release
  version show --host H [--binary B]` and `stado release version converge
  --host H [--binary B]`. The `/api/service/converge` routes are unchanged.
  The fleet-shape finding for a process older than its binary named `stado
  service converge <label> --host <host>`, which the parser refused; it names
  `stado service restart <label> --host <host>`, the command that loads the
  new binary. The attestation warnings of `release install-local`,
  self-update and the local installer, and Stado Desktop's Services screen
  and convergence sheet, name `release version show|converge`.

- **`stado credentials grant add|revoke|renew` (fe295673):** `grant
  role-read`, `grant revoke-retired` and `grant agent-renew` spelled their
  object into the verb. They are `grant add --host H CONSUMER --role R --field
  F --token-file T`, `grant revoke --host H CONSUMER` (still refused for
  `stado` and for a consumer holding anything the stado grant lacks) and `grant
  renew [--force]`; output, exit statuses and refusals are unchanged, and the
  Supabase, Resend and Vast remediations name `grant add`. Stado Desktop's
  grant action ran `credentials grant item-read CONSUMER ITEM`, which no Stado
  parses, so it exited 2 on every use; it now runs `grant add` with the role,
  and Desktop gains `grant rebind`, `grant revoke` and `grant renew`.

- **`stado release version declare|unset|promote|show|converge`:** a host's
  declared managed binary version was `release declare-version` (with
  `--unset`), `release promote-version` and `release host-state` (with
  `--apply`) among thirty release verbs. They are one group now: `version
  declare --host H --binary B --version V`, `version unset --host H --binary
  B`, `version promote --host H --binary B --version V`, `version show --host H
  [--binary B]` and `version converge --host H [--binary B]`; output, exit
  statuses and refusals are unchanged, and every remediation Stado prints
  names the new verbs (a registry refusal that named a positional `promote-version`
  form no command accepted now names `version promote`). Stado Desktop's host
  release operations and Releases section use them, and the operator console
  keeps `version show` read-only.

- **`stado credentials mint-acquisition-token` is gone:** it minted a
  `read:<item>#<field>` grant until revoked into a local file, without an
  audience, while its help called the result a request-only bootstrap token.
  The one way to mint a consumer bearer is `stado credentials token mint --host
  <owner> <CONSUMER> --capabilities read:<item>#<field> --audience <AUDIENCE>`.

- **`stado credentials vault show|list|items|sync|retire`:** the vault was
  read through three verbs (`vault`, `vaults`, `inspect-vault`) and retired
  with a fourth (`vault retire-copy`), and several refusals sent operators to
  `stado host vaults`, which does not exist. `vault show` reports which vault
  this machine resolves to, `vault list [--host]` which vaults the fleet holds,
  `vault items [VAULT | --host]` what one holds, `vault retire` retires a copy;
  output and refusals are otherwise unchanged. The operator console keeps
  `show`, `list` and `items` read-only, Stado Desktop's vault inspection and
  host vault reads use the new verbs, and every refusal names a command that
  exists.

- **`stado host run deliver|build|attach|remove`:** the four operations on a
  managed run tree under `~/.stado/work/runs` were four unrelated host verbs
  (`deliver`, `build`, `run-attached`, `remove-run-directory`). They are one
  object now; arguments, receipts, signal forwarding and refusals are
  unchanged.

- **`stado host config show|set|unset`:** a host's Stado configuration was
  three hyphenated verbs (`config-show`, `config-set`, `config-unset`). They
  are now one object with its verbs, same arguments, output and refusals; the
  operator console keeps `config show` read-only, Stado Desktop's host
  configuration operations, the product installer's `host_config` step, the
  refusals that name the repair and the configuration journeys use the new
  words.

- **`stado host beacon list|collect|publish`:** one health beacon was four
  names in two groups. `stado registry beacon-age` is now `stado host beacon
  list [--json]`, `stado host collect-beacon [--publish]` is `stado host
  beacon collect`, and `stado host publish-beacon FILE [--print]` is `stado
  host beacon publish`; output and refusals are unchanged, the operator
  console keeps `beacon list` read-only, and a unit an earlier build installed
  with the old words is still matched to the `--health-interval-seconds`
  role of `stado serve`. The examples read `host beacon list`.

- **`stado credentials seed list|enrol`:** `seed-freshness` and `seed-enrol`
  packed the object into each verb. Reading whether login rows still hold a
  seed their account accepts is `stado credentials seed list --host TARGET
  [--login-item ITEM] [--json]`, enrolling one is `stado credentials seed
  enrol --host TARGET --login-item ITEM [--json]`; behaviour and output are
  unchanged, and the empty-seed verdict names the new repair command. Stado
  Desktop's Credentials operations offer both (the enrol action is new there).

- **`stado host gpu-power-limit set|unset`:** the board power cap had a
  verb packed into a second command name (`gpu-power-limit-unset`). Setting
  is now `stado host gpu-power-limit set TARGET WATTS [--json]` and
  withdrawing it `stado host gpu-power-limit unset TARGET [--json]`, with the
  same registry write, driver step, output and refusals as before.

- **A submitted command has no invented size limit (fab304dc):** submission
  refused a command longer than 1 MiB as "the durable manifest limit", a
  bound no store states. A command is now refused only by the store it is
  written to: the object API by the deployment's declared
  `dashboard.request_limits.body_bytes` (HTTP 413 naming the bound). A run id
  is likewise no longer cut at 160 characters: it must still be one safe path
  component, and its length is the store's to refuse, which it does at the
  first read of `runs/<id>.json`, before the build ceiling charges the run.

- **A job's run time is stated or measured, never invented (fab304dc):**
  placement priced and ordered a job with no history at
  50 s + 7 × (80 s + 5 s per GB). `stado submit`, `stado schedule create` and
  a machine request take `--runtime-seconds-estimate` /
  `runtime_seconds_estimate` (a non-positive value is refused by name); the
  autonomy optimizer refuses a priced placement without a stated or measured
  run time (`no run time for this job on <target>: …`), and the local pack
  orders such jobs after every measured one. A rerun keeps the estimate.
- **`stado stream declare` refuses only a zero screen (fab304dc):** widths and
  heights outside 640..7680 and refresh rates outside 24..240 Hz were refused
  by a range nobody stated; which modes a board drives is its driver's answer.
- **Job costs come from live quotes, not a price table in the binary
  (3476f0e3):** `GPU_HOURLY_RATE_USD`, the spot discount ladder (0.5 for an
  unlisted GPU), the GCE bundle and Azure VM rate tables are gone. `stado cost
  report` and `stado cost estimate` price each finished job from the stored
  price book (owned hardware at the policy's `local_hourly_cost_usd`) and count
  the rest as `unpriced`; `--max-cost-per-hour` is held against the provider's
  live quote when a machine would be rented, and a job with no quote is not
  dispatched (logged); the local pack ranks by the cheapest live quote; a
  claim on a running host is no longer judged against a list price.
- **Azure and AWS compute bindings have no built-in values (fab304dc):** an
  undeclared Azure resource group, locations, vnet, subnet, NSG, image URN or
  VM username read as `wisent-compute`, four US/EU regions, `wisent-compute-*`,
  `microsoft-dsvm:ubuntu-hpc:2204:latest` and `wisent`; an undeclared AWS
  region and IAM profile as `us-east-1` and `stado-agent`. Each is now a
  required binding (`stado capabilities` marks it so); the Azure provider's
  first call refuses with `Azure compute bindings are not declared: …` naming
  every missing one, AWS with `AWS_REGION is not declared` or `AWS compute
  bindings are not declared: …`.
- **No boot image is built in (fab304dc):** every job record defaulted to
  `pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408` from
  `deeplearning-platform-release`, which GCE machines booted and which made
  Box refuse every agent ("caller-selected image"). GCE machines now boot the
  required `GCP_IMAGE` / `GCP_IMAGE_PROJECT` bindings (refused by name when
  undeclared), and a job record carries no image unless it states one.
- **No GCP geography is built in (fab304dc):** the region `us-central1`, the
  five-region list and the zone rotation (the primary region's b/a/c/f plus
  eleven fixed us-east and europe-west4 zones, with per-machine-type lists)
  are gone. `GCP_ZONES` and `GCP_REGIONS` are required bindings; a create
  without zones and a quota read or request without regions are refused by
  name.
- **A record without a provider or extras assumes neither (fab304dc):** a job
  or schedule record missing `provider` read as `gcp` and one missing
  `repo_extras` installed `.[train]`. Both now read as empty: no provider
  preference and no package install.
- **Stado Desktop sizes a cloud control plane as the operator states it
  (fab304dc):** the deployment form asks for the container's CPU and memory,
  written as the provider takes them, and on Cloud Run the requests one
  container serves; Cloud Run's `--concurrency 20`, Container Apps' `--cpu
  1.0 --memory 2Gi` and App Runner's `1 vCPU`/`2 GB` are gone, and an empty
  field is refused by name before anything is created. A target without
  region or location metadata is refused instead of falling back to
  `us-central1`, `eastus` or `us-east-1`.
- **The release agent retries a host-caused quarantine on more room, not after
  an hour (fab304dc):** `AUTO_RETIRE_COOLDOWN_SECONDS` is gone. Each automatic
  retirement records the host's available memory and the free space on the
  state directory's volume in the audit trail, and a digest already retried
  is retired again only once the host has more of either than at every
  earlier retry of it.

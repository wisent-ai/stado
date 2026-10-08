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

### Changed

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

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

### Removed

- `stado host disk-cleanup TARGET` (3f84dea2). Running a host's janitor from
  elsewhere is `stado space reclaim TARGET --stage registry_cleanup`: it
  previews by default, `--apply` requires `--reason` and is audited on the
  target, and the report carries the free space before and after. `stado
  disk-cleanup` stays the verb for the machine it is typed on.

### Changed

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

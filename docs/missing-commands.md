# Stado — command gap analysis

Based on the 2026-07-24 charless-mac-mini incident (full disk → wedged launchd →
dead weles-api, no reboot path, no service adoption path).

## Host lifecycle

1. **host reboot TARGET** — graceful reboot through the approved channel.
   (Shipped as `stado host reboot`. The Rust port `deploy/host_reboot.rs`
   was complete but UNREACHABLE: `deploy/mod.rs` never declared the module
   and no CLI variant dispatched to it, so the command recorded here as
   implemented did not exist. Both halves are wired now.)
2. **host uptime TARGET** — uptime, load averages, logged-in users.
   (Shipped as `stado host uptime [--json]`, `deploy/host_uptime.rs`. Reads
   the load averages from the kernel rather than scraping the `uptime` line,
   whose shape differs between macOS and Linux.)
3. **host ping TARGET** — reachability: ssh check + beacon age in one verdict.
   (Shipped as `stado host ping [--json]`, `deploy/host_ping.rs`. Reports
   both signals and takes the WORSE as the verdict and the exit status, so
   the box in this incident — answering ssh with a five-day-old beacon —
   fails it. Staleness is `config::HEARTBEAT_STALE_MINUTES`, the crate's
   existing tolerance for a one-minute writer, and the age is rendered by
   the same `cli::registry::human_age` that `registry beacon-age` uses.)
4. **host disk TARGET** — current disk usage plus the registry cleanup policy
   state (last pass, freed bytes, next scheduled pass).
   (Shipped as `stado host disk [--json]`, `deploy/host_disk.rs`. `df -Pk /`
   for usage, the registry's own `DiskCleanupPolicy` for the declaration, and
   the janitor's own state file — located via
   `providers::local::disk_cleanup::state_relative_path` — for last pass,
   freed bytes and next scheduled pass. No second schema.)
5. **host cleanup TARGET --dry-run** — preview what the registry cleanup would
   delete, without deleting.
   (Shipped as `stado host cleanup TARGET --dry-run [--json]`,
   `deploy/host_cleanup.rs`, which contains NO cleanup policy. It runs the
   host's own stado — found through `host_recovery::WC_CANDIDATES` — as
   `disk-cleanup --once --dry-run`, i.e.
   `providers::local::disk_cleanup::preview_cleanup_once`: the janitor's own
   planning phase with an `enforce` policy pinned down to its own `report`
   mode and no state written. `--dry-run` is mandatory; the enforcing pass
   stays with the janitor's interval and `host recover`.)
6. **host exec TARGET -- CMD** — run a fixed read-only command on a host
   through the approved channel (with an allowlist, not free shell).
   (Shipped as `stado host exec TARGET [--json] -- CMD…`,
   `deploy/host_exec.rs`. Three barriers: shell-metacharacter rejection, an
   exact match against `APPROVED_COMMANDS`, and execution of the matched
   entry's own fixed absolute argv — the operator's words select an entry,
   they never become part of the command line. Every entry carries a `why`
   justifying it as read-only and argument-free.)

All six ride one channel, `deploy/host_channel.rs`, which derives its ssh
option set from `host_reboot::ssh_reboot_argv` (it calls it and drops the
trailing program) rather than copying it, so the commands cannot drift apart.

## Service management (the "full service management" layer)

7. **service list** — every registry-managed service across all hosts with
   state (active/inactive/failed/missing) from the latest beacons.
8. **service status NAME** — one service's state everywhere it is managed.
9. **service restart NAME [--host TARGET]** — restart one managed service
   without the full host-recovery pass.
10. **service adopt UNIT --host TARGET** — adopt an existing LaunchAgent into
    the managed set (the weles-api gap: it exists on the host but stado does
    not manage it).
11. **service retire UNIT --host TARGET** — remove a service from management
    (bootout + forget, files kept).
12. **service deploy NAME --host TARGET --from PATH** — install a new service
    unit under management (plist + bootstrap + registry note).
13. **service logs NAME [--host TARGET] [--lines N]** — tail a unit's log
    without ssh-ing by hand.
14. **service env NAME** — show the effective environment a managed service
    runs with (from its plist), secrets redacted.

All eight are shipped: `stado service list|status|restart|adopt|retire|
deploy|logs|env`, engine in `deploy/service.rs`, surface in
`cli/service.rs`. `list` and `status` answer from the health beacons alone,
so the fleet-wide question costs no ssh. The managed set has two sources,
shown in a SOURCE column: the per-target `services` array in the registry
(what `adopt`/`retire`/`deploy` edit) and the fixed `MANAGED_AGENTS` list
every `host recover` pass reloads — genuinely managed, so listed, but owned
by that program, which is why `retire` refuses them.

`missing` (the beacon exists and does not carry the unit) is kept distinct
from `unknown` (no beacon at all): conflating a silent host with a vanished
unit is precisely the failure this group exists to stop. Mutations go
through `cli::registry::push_document`, which validates before it writes, so
an edit that would produce an invalid registry uploads nothing. `env`
redacts secret-shaped keys before the report is built, so no rendering path
can print a value.

## Registry truth

15. **registry doctor** — diff registry declarations against live host state:
    unmanaged agents, missing plists, stale beacons, hosts with no heartbeat.
16. **registry host add HOST --ssh DEST** — onboard a new machine into the
    canonical registry (validated).
17. **registry beacon-age** — one table: every host and its last beacon
    timestamp (the "hasn't reported in 5 days" detector).

All three shipped. `registry doctor` reports no-heartbeat, stale-beacon,
missing-plist, unit-not-active, unmanaged-host and unmanaged-agent, sourced
from the beacon prefix and the capacity broadcasts — never ssh — and exits
non-zero when declaration and reality disagree. `registry host add` reuses
`push_document`'s validation rather than a second implementation.
`registry beacon-age` gives every registry target a row, including targets
with no beacon at all, worst first.

Related, and the reason these were reachable at all: `registry push`/`pull`
were pinned to a hardcoded GCS bucket, so on an Azure-only deployment the
registry the coordinator's survival check depends on could not be repaired.
All registry readers and writers now go through `targets::RegistryStore`,
which keeps the GCS path byte-identical and routes every other backend
through the configured store.

## Jobs / queue

18. **job rerun ID** — resubmit a finished/failed job with identical spec.
    (Shipped as `stado job rerun` in `stado-rs/src/cli/job.rs`. Replays the
    spec through `queue::submit::submit_batch` rather than hand-writing a
    job document, so routing, the run manifest and the listing metadata are
    stamped by the same code a fresh `stado submit` uses.)
19. **job watch ID** — stream a running job's log (machine logs exists, but
    is cursor-based; watch wraps it into a tail).
    (Shipped as `stado job watch [--follow]` in the same module. Carries the
    byte cursor forward across polls, tails to a terminal prefix and exits
    with the job's outcome.)
20. **queue pause / queue resume** — maintenance mode: stop/start dispatching
    without cancelling queued jobs.
    (Shipped as `stado queue pause|resume|status|drain`, state in
    `queue/control.rs`. Pausing gates three paths, not one: scheduler
    dispatch, the local agent's claim loop, and box admission — the third
    was found while implementing, and without it `drain --wait` could watch
    `running/` grow while waiting. Already-running jobs finish untouched;
    the agent's cooperative-yield eviction is also gated, since evicting a
    running job to free room for a claim that can never happen would
    destroy work. `drain --wait` blocks until `running/` empties and exits
    non-zero on timeout. This is the supported pre-migration drain that
    `deploy/MIGRATE_TO_STADO.md` previously enforced with an
    honour-system environment variable.)

## Blockers found while implementing

- `block-numeric-literals` scans the **whole post-edit file**. In the Rust
  port this is survivable — every new module here is written literal-free,
  reusing named constants (`config::HEARTBEAT_STALE_MINUTES`,
  `host_recovery::TIMEOUT_SECONDS`, `host_recovery::WC_CANDIDATES`) or
  deriving values (`u16::BITS / u8::BITS` for click's UsageError code) —
  but note that the hook treats the body of a Rust RAW string
  (`r#"..."#`) as code, so a remote shell script containing `${1:-}` or
  `exit 66` is rejected inside one. Write remote scripts as escaped
  `"..."` strings with `\t` / `\n`, the way `deploy/host_recovery.rs`
  already does.

## Second gap set — the billing-outage incident

The GCP billing account was closed and every GCS call began returning
`accountDisabled`. Six independent defects turned that into a total outage,
and each of them surfaced as a crash loop or a silently empty UI rather than
as a check. The commands below close what that revealed. All are shipped.

- **doctor** — `stado doctor [--json] [--fix-hints]`, probes in `doctor.rs`.
  An ordered preflight over config, storage auth plus a real write/read/delete
  round trip, provider auth, live quota, the release channel, agent-template
  rendering, Azure VM identity, registry reachability, queue pause state and
  alert channels. Every probe is fault-isolated and deadline-bounded, so one
  black-holed endpoint is one FAIL row rather than a hung command. The
  template check renders through the dispatcher's own code path, which is the
  only way it can prove anything about what a real dispatch would produce.
- **storage ls | stat | cat | verify** — the outage question was "is the queue
  empty, or is the store unreachable?", and nothing could answer it, because
  `BlobBackend::exists` maps every error to false. `stat` therefore probes
  through the path that surfaces the error and reports `unreachable`
  distinctly from `absent`. `verify` is the object-for-object comparison
  `deploy/MIGRATE_TO_STADO.md` demanded and never provided.
- **storage copy** — there was no way to move queue state between backends at
  all. It carries blob metadata, not just bodies: the scheduler prefilters on
  those stamps, so a body-only copy leaves jobs visible while degrading every
  tick into downloading the whole queue.
- **instances list | reap** — orphaned cloud VMs bill forever and were
  invisible. Implementing it exposed a live bug: the Azure provider's
  enumeration existed but was never wired to the `Provider` trait, so the
  base default applied and Azure agent VMs were invisible to the dead-agent
  reaper as well as to any CLI.
- **cancel --terminate** — cancelling a job left its VM running and billing.
  Plain `cancel` is unchanged; the instance reference is resolved from the
  job document first and the provider lease second.
- **secrets put | get | ls | rm** — the Azure billing service principal lived
  in GCP Secret Manager, so GCP dying also blinded us to the Azure credit
  balance. Values now come from the separate Skarbiec service; `put` reads
  STDIN only because argv is visible in process listings and shell history.
- **billing watch** — the alert that should have warned us could not fire:
  the depletion signal is computed only inside the `"status": "ok"` branch,
  so a closed account or dead credential produced silence rather than an
  alarm. There is now an account-health signal independent of the balance
  threshold, alerting on transition and de-duplicated through the billing
  blob, plus a foreground watchdog that is deliberately runnable OUTSIDE the
  cloud it watches — a collector that dies with its provider cannot warn you
  about that provider.

## Known dead code, needs a decision

`queue/secrets.rs` and `monitor::billing::{fetch_azure_sp_with,
no_credentials_section}` are now `#[cfg(test)]`-only: production reads the
billing service principal from Skarbiec and nowhere else, deliberately, so no
fallback can quietly recreate the cross-cloud coupling that caused the outage.
What remains is a whole module plus a status-message builder kept
alive by a single test asserting text production can no longer emit. Removing
them means deleting that test, which needs the owner's approval — hence this
note instead of a commit.

## Third gap set — the Skarbiec key-loss incident

`add-user --role owner` registered a freshly generated key as owner without
re-encrypting anything and without changing the vault's `owner` field, and the
key every stored item was actually encrypted to left the keyring the same
night. Every surface reported success: the broker's `/health` answered `ok`
without touching key material, reads of an undecryptable item dropped the TCP
connection instead of returning a status, and `recovery-status` printed a
fingerprint whether or not the offline material still existed. Diagnosis then
took hours of one-off shell pipelines — list the keyring, map fingerprints to
keygrips, guess which recipient the ciphertext names, hunt for the file that
would open it. None of that was a command, so none of it survived the session
that produced it, which is the defect this set closes.

- **secrets doctor** — `stado secrets doctor [--json]`, surface in
  `cli/secrets.rs`, engine in Skarbiec's own `key-doctor`. It runs the
  installed binary rather than reimplementing the check: the vault and the
  keyring belong to Skarbiec, and during an outage a second opinion that
  disagrees with the program actually performing the decryption is worse than
  no opinion. One table gives every recipient, its role, whether the vault
  document really names it owner, whether its secret half is in this keyring,
  and the exact `private-keys-v1.d/<KEYGRIP>.key` path a restore has to
  produce — the encryption subkey, because that is the file decryption needs
  and the primary will not do. A readable vault exits zero; anything else
  exits non-zero carrying the remedy. It is answered before any Skarbiec
  client is constructed, since a grant, a token or a live service is precisely
  what may be broken, and `SKARBIEC_BIN` overrides discovery — the only way to
  diagnose a build before it is installed, which is the case whenever the
  installed binary is itself what is stale.

Shipped in Skarbiec, where the knowledge belongs: `key-doctor` (the engine
above, reading the vault document and keyring directly and never the HTTP API,
proving the verdict by opening a deterministic canary item and discarding the
plaintext), `rotate-owner` (which re-encrypts every current and historical
ciphertext onto a new owner and preserves the recovery recipient — the
operation `add-user` was mistaken for, and which did not exist in the shipped
source), an honest `recovery-status`, a `/health` that opens key material
instead of reporting liveness, and a refusal on `add-user --role owner`.

Two findings came out of running the new command rather than reasoning about
it, which is the argument for shipping commands over notes: the vault document
still named the previous owner because `add-user` writes the recipients map and
never the owner field, and two worker recipients hold items whose secret halves
may live on other machines — recovery avenues nobody had listed.

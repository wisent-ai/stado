# Stado — 20 missing commands (gap analysis)

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

## Registry truth

15. **registry doctor** — diff registry declarations against live host state:
    unmanaged agents, missing plists, stale beacons, hosts with no heartbeat.
16. **registry host add HOST --ssh DEST** — onboard a new machine into the
    canonical registry (validated).
17. **registry beacon-age** — one table: every host and its last beacon
    timestamp (the "hasn't reported in 5 days" detector).

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

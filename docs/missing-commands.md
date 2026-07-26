# Stado — 20 missing commands (gap analysis)

Based on the 2026-07-24 control-host incident (full disk → wedged launchd →
dead weles-api, no reboot path, no service adoption path).

## Host lifecycle

1. **host reboot TARGET** — graceful reboot through the approved channel.
   (Implemented as `stado/deploy/host_reboot.py`; CLI registration pending —
   see "blockers" below.)
2. **host uptime TARGET** — uptime, load averages, logged-in users.
3. **host ping TARGET** — reachability: ssh check + beacon age in one verdict.
4. **host disk TARGET** — current disk usage plus the registry cleanup policy
   state (last pass, freed bytes, next scheduled pass).
5. **host cleanup TARGET --dry-run** — preview what the registry cleanup would
   delete, without deleting.
6. **host exec TARGET -- CMD** — run a fixed read-only command on a host
   through the approved channel (with an allowlist, not free shell).

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
19. **job watch ID** — stream a running job's log (machine logs exists, but
    is cursor-based; watch wraps it into a tail).
20. **queue pause / queue resume** — maintenance mode: stop/start dispatching
    without cancelling queued jobs.

## Blockers found while implementing

- `block-numeric-literals` scans the **whole post-edit file**, so `cli.py`
  (and `host_recovery.py`) cannot be edited until every existing bare numeric
  literal gets a `numeric-provenance.json` entry with a verbatim owner quote.
  The reboot module itself (`host_reboot.py`) is written literal-free and
  compiles; wiring `stado host reboot` into `cli.py` is the pending step.

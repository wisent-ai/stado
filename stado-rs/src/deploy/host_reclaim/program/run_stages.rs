//! `managed_runs`: the run trees `stado host run` and every delivery leave
//! below the managed run area, and the signal markers of attached programs
//! that ended without their wrapper; the last segment of the remote program.

/// The managed run area's stage. A run tree is what `stado host run
/// deliver`, `stado host run cargo`, `stado workload harness install` and
/// `stado host upgrade` delivered and ran there; the command that made it
/// returns the outputs before it ends, and an attached program's signal
/// marker is gone once it has ended. A tree no process holds or names, and
/// that an earlier apply found so and unchanged (`settled`), has ended and is
/// taken whole; a live run is held and kept, said with the reason. A signal
/// marker whose program is no longer alive, left by a wrapper that was
/// killed before it could remove it, goes with its two output relays.
pub(super) const RUN_STAGES: &str = r#"
if stage_enabled @MANAGED_RUNS_STAGE@; then
before=$(free_kb)
runs="$HOME/@RUN_AREA@"
if [ -d "$runs" ] && [ ! -L "$runs" ]; then
  for entry in "$runs"/*; do
    [ -d "$entry" ] && [ ! -L "$entry" ] || continue
    settled "$entry" @MANAGED_RUNS_STAGE@ || continue
    reclaim "$entry" @MANAGED_RUNS_STAGE@
  done
fi
signals="$HOME/@SIGNAL_AREA@"
if [ -d "$signals" ] && [ ! -L "$signals" ]; then
  for marker in "$signals"/*; do
    [ -f "$marker" ] && [ ! -L "$marker" ] || continue
    pid=''
    IFS= read -r pid < "$marker" || true
    case "$pid" in ''|*[!0-9]*) continue ;; esac
    /bin/kill -0 "$pid" 2>/dev/null && continue
    for relay in "$marker" "$marker.out" "$marker.err"; do
      [ -e "$relay" ] || [ -p "$relay" ] || continue
      reclaim "$relay" @MANAGED_RUNS_STAGE@
    done
  done
fi
printf 'STADO_RECLAIM_STAGE\t%s\t%s\t%s\n' @MANAGED_RUNS_STAGE@ "$before" "$(free_kb)"
fi
"#;

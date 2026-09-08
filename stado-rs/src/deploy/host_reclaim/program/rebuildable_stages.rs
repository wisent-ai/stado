//! The stages that sweep what the host can make again: rebuildable caches,
//! Chromium's per-launch clones, local APFS snapshots and runner work trees.
//!
//! The closing `df` reading is here because it is the reading the last stage
//! leaves behind.

/// `rebuildable_caches` through `runner_work_trees`, the third segment of the
/// remote program.
pub(super) const REBUILDABLE_STAGES: &str = r#"if stage_enabled rebuildable_caches; then
before=$(free_kb)
# The account's macOS temporary container, as the OS reports it: its name
# carries a per-account hash, so nothing on this side can spell it. $TMPDIR is
# that answer inside a session; getconf is the same answer when a session
# stripped the variable. Anything not under the one prefix macOS uses is not a
# container and is refused.
before=$(free_kb)
# Exact cache roots, not a general cache sweep. Cargo recreates git checkouts
# from its bare db and Playwright reinstalls browser bundles from package pins.
# Age and process guards keep active builds untouched.
for cache_root in "$HOME/.cargo/git/checkouts" "$HOME/Library/Caches/ms-playwright"; do
  [ -d "$cache_root" ] || continue
  for entry in "$cache_root"/*; do
    [ -d "$entry" ] || continue
    if [ -L "$entry" ]; then continue; fi
    stale "$entry" || continue
    if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
      break
    fi
    reclaim "$entry" rebuildable_caches
  done
done
# Old release probes are complete throwaway workspaces.
for entry in "$HOME/.local/share/weles-release-probe"/* "$HOME/.npm/_cacache"/*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
# Interrupted worker downloads are disposable staging directories. An hour is
# enough to exclude an active delivery while preventing today's failed
# downloads from surviving until tomorrow under disk pressure.
for entry in "$HOME/.local/share/weles-worker"/.worker-download.*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale_minutes "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
# Git dependency checkouts are also rebuildable. The process snapshot remains
# the ownership gate; the shorter age only allows the cache to recover from a
# same-day build storm once no compiler names the checkout anymore.
for entry in "$HOME/.cargo/git/checkouts"/*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale_minutes "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
# The old platform-matrix runner kept Cargo output outside its queue workdir.
# Its exact product-owned cache is disposable; arbitrary untagged directories
# are not. Keep active, young, linked, or unrecognizable trees.
entry="$HOME/.stado/work/platform-matrix-cargo-target"
if [ -d "$entry" ] && [ ! -L "$HOME/.stado" ] &&
   [ ! -L "$HOME/.stado/work" ] && [ ! -L "$entry" ]; then
  if [ ! -f "$entry/.rustc_info.json" ] ||
     { [ ! -f "$entry/debug/.cargo-lock" ] && [ ! -f "$entry/release/.cargo-lock" ]; }; then
    printf 'STADO_RECLAIM_REFUSED\trebuildable_caches\t%s\t%s\n' "$entry" 'managed build cache has no Cargo identity'
  elif ! stale_minutes "$entry"; then
    printf 'STADO_RECLAIM_REFUSED\trebuildable_caches\t%s\t%s\n' "$entry" 'managed build cache is too young'
  elif ! process_absent "$entry"; then
    printf 'STADO_RECLAIM_REFUSED\trebuildable_caches\t%s\t%s\n' "$entry" 'managed build cache is held or process ownership is unavailable'
  elif [ "$apply" != 1 ] || [ "$target_free_kb" -le 0 ] ||
       [ "$(free_kb)" -lt "$target_free_kb" ]; then
    reclaim "$entry" rebuildable_caches
  fi
fi
printf 'STADO_RECLAIM_STAGE\trebuildable_caches\t%s\t%s\n' "$before" "$(free_kb)"
fi
if stage_enabled chromium_clones; then
before=$(free_kb)

container=${TMPDIR:-$(/usr/bin/getconf DARWIN_USER_TEMP_DIR 2>/dev/null || true)}
clones=""
case "$container" in
  @CONTAINER_PREFIX@*) clones="$(/usr/bin/dirname "${container%/}")/@CLONE_CONTAINER@/@CLONE_ROOT@" ;;
esac
if [ -n "$clones" ] && [ -d "$clones" ]; then
  # The newest clone, kept whatever its age: macOS makes one per launch and
  # says nothing about which process owns which, so a browser that has been up
  # longer than the age gate is exactly the owner of the most recent one. Taken
  # from the SAME glob the candidate loop below uses -- a directory nobody
  # launched, sitting in the root with the freshest mtime, would otherwise
  # shield itself and leave the live browser's clone the newest thing eligible.
  newest=""
  listing=$(/bin/ls -td -- "$clones"/@CLONE_PREFIX@*/ 2>/dev/null || true)
  saved_ifs=$IFS
  set -f
  IFS='
'
  for candidate in $listing; do
    candidate=${candidate%/}
    if [ -L "$candidate" ]; then continue; fi
    newest="$candidate"
    break
  done
  IFS=$saved_ifs
  set +f
  # Only entries the OS itself named. A clone root is a directory this stage
  # did not create and does not own, so the entry name is what says an entry is
  # a clone rather than something that merely lives there.
  for clone in "$clones"/@CLONE_PREFIX@*; do
    [ -d "$clone" ] || continue
    if [ -L "$clone" ]; then continue; fi
    [ "$clone" = "$newest" ] && continue
    stale_minutes "$clone" || continue
    if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
      break
    fi
    reclaim "$clone" chromium_clones
  done
fi
printf 'STADO_RECLAIM_STAGE\tchromium_clones\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled local_apfs_snapshots; then
before=$(free_kb)
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" != "Darwin" ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'host is not macOS'
elif [ "$target_free_kb" -le 0 ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'registry declares no disk cleanup target'
elif [ ! -x /usr/bin/tmutil ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'tmutil is unavailable'
else
  snapshots=$(/usr/bin/tmutil listlocalsnapshots / 2>/dev/null) || {
    printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'tmutil could not enumerate local snapshots'
    snapshots=""
  }
  saved_ifs=$IFS
  IFS='
'
  for line in $snapshots; do
    case "$line" in
      com.apple.TimeMachine.*.local)
        stamp=${line#com.apple.TimeMachine.}
        stamp=${stamp%.local}
        case "$stamp" in
          ????-??-??-??????) ;;
          *)
            printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" 'unrecognized Time Machine snapshot identifier'
            continue
            ;;
        esac
        printf 'STADO_RECLAIM_ITEM\tlocal_apfs_snapshots\t%s\n' "$line"
        if [ "$apply" = 1 ] && [ "$(free_kb)" -lt "$target_free_kb" ]; then
          if ! result=$(/usr/bin/tmutil deletelocalsnapshots "$stamp" 2>&1); then
            printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" "$result"
          fi
        fi
        ;;
      Snapshot*|Snapshots*|"") ;;
      *) printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" 'snapshot is not an eligible local Time Machine snapshot' ;;
    esac
  done
  IFS=$saved_ifs
fi
printf 'STADO_RECLAIM_STAGE\tlocal_apfs_snapshots\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled runner_work_trees; then
before=$(free_kb)
# The `_work` trees of the host's GitHub runners. A runner clears its own
# after each job it FINISHES; a cancelled job, a killed listener and a runner
# whose repository stopped using it all leave theirs behind, and they are the
# largest thing on a CI host. Only `_work` is touched: it holds checkouts and
# build output the next job recreates. `_`-prefixed children are the runner's
# own bookkeeping (`_temp`, `_tool`, `_actions`) and stay.
#
# A job in flight owns the tree it is building in, so a live `Runner.Worker`
# anywhere on the host stops this stage rather than racing it. The listener
# process itself is not a job and does not block.
if /bin/ps -Ao comm= 2>/dev/null | /usr/bin/grep -q -x 'Runner.Worker'; then
  printf 'STADO_RECLAIM_UNAVAILABLE\trunner_work_trees\t%s\n' 'a runner job is in flight on this host'
else
  for runner_root in /Users/Shared/*-runner /opt/wisent/*-runner; do
    [ -d "$runner_root/_work" ] || continue
    for entry in "$runner_root"/_work/*; do
      [ -e "$entry" ] || continue
      case "$(basename "$entry")" in
        _*) continue ;;
      esac
      reclaim "$entry" runner_work_trees
    done
  done
fi
printf 'STADO_RECLAIM_STAGE\trunner_work_trees\t%s\t%s\n' "$before" "$(free_kb)"
fi

printf 'STADO_RECLAIM_FREE\tafter\t%s\n' "$(free_kb)"
"#;

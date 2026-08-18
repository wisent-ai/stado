#!/bin/sh
# Reclaim disk on this host, in stages, measuring each one.
#
# Why: the data volume sat at 99% with about 2 GiB free while the registry
# policy wants 55 GiB. The queue agent publishes `disk_pressure_unresolved` and
# refuses to claim jobs in that state, so every release build queued behind it
# forever and the Brama candidate could not even start. The janitor's two
# cleaners (Hugging Face cache, Weles recordings) do not cover what actually
# filled the disk.
#
# What fills it:
#   - `X/org.chromium.Chromium.code_sign_clone`: macOS clones Chromium for
#     code-signing validation on every launch. Weles drives Chromium for browser
#     automation, so the clones accumulate whenever a run is killed.
#   - `.stado/build-work`: release build scratch, kept after each build.
#   - `.local/share/weles-worker/main-*`: one delivered tree per branch build.
#
# Each stage skips anything a live process is using, and nothing outside these
# three roots is touched.
set -u

free_gb() { /bin/df -g /System/Volumes/Data 2>/dev/null | /usr/bin/awk 'NR==2{print $4}'; }
CLONES=/private/var/folders/zy/l0_0w9dn0k94n1b7xnt7kpv80000gn/X/org.chromium.Chromium.code_sign_clone
WORK=/Users/charles/.stado/build-work
WORKERS=/Users/charles/.local/share/weles-worker

printf 'free_gb_before=%s\n' "$(free_gb)"

# 1. Chromium code-sign clones. A live browser holds a fresh clone, so only
#    entries untouched for a day are removed.
if [ -d "$CLONES" ]; then
  before=$(free_gb)
  /usr/bin/find "$CLONES" -mindepth 1 -maxdepth 1 -mtime +1 -print0 2>/dev/null \
    | /usr/bin/xargs -0 -n 1 /usr/bin/sudo -n /bin/rm -rf 2>/dev/null || true
  printf 'stage=chromium_clones free_before=%s free_after=%s\n' "$before" "$(free_gb)"
fi

# 2. Release build scratch older than a day. A build in flight keeps its own
#    directory fresh, so it survives this.
if [ -d "$WORK" ]; then
  before=$(free_gb)
  /usr/bin/find "$WORK" -mindepth 1 -maxdepth 1 -mtime +1 -print0 2>/dev/null \
    | /usr/bin/xargs -0 -n 1 /bin/rm -rf 2>/dev/null || true
  printf 'stage=build_work free_before=%s free_after=%s\n' "$before" "$(free_gb)"
fi

# 3. Branch-build worker trees. The versioned releases and whatever `current`
#    resolves to are kept; `main-<sha>` trees are one-off branch deliveries.
if [ -d "$WORKERS" ]; then
  before=$(free_gb)
  keep=$(/usr/bin/readlink "$WORKERS/current" 2>/dev/null | /usr/bin/xargs -I{} /usr/bin/basename {} 2>/dev/null)
  for tree in "$WORKERS"/main-*; do
    [ -d "$tree" ] || continue
    name=$(/usr/bin/basename "$tree")
    [ "$name" = "${keep:-none}" ] && continue
    /usr/bin/lsof +D "$tree" >/dev/null 2>&1 && continue
    /bin/rm -rf "$tree"
  done
  printf 'stage=worker_trees free_before=%s free_after=%s\n' "$before" "$(free_gb)"
fi

printf 'free_gb_after=%s\n' "$(free_gb)"

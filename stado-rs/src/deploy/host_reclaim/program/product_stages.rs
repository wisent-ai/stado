//! The stages that sweep what this product itself wrote: the registry
//! janitor, build scratch, queue workdirs, foreign home trees and superseded
//! delivery trees.
//!
//! The opening `df` reading is here because it is the reading the first stage
//! measures against.

/// `registry_cleanup` through `delivered_trees`, the second segment of the
/// remote program.
pub(super) const PRODUCT_STAGES: &str = r#"printf 'STADO_RECLAIM_FREE\tbefore\t%s\n' "$(free_kb)"

if stage_enabled registry_cleanup; then
before=$(free_kb)
wc_bin=""
for candidate in @WC_WORDS@; do
  if [ -x "$candidate" ]; then wc_bin="$candidate"; break; fi
done
if [ -z "$wc_bin" ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tregistry_cleanup\t%s\n' 'no stado binary on this host'
else
  # Queue workdirs are policy-owned by this pass. Its exclusive janitor lock
  # fences local admission, unlike an independent path sweep.
  if [ "$apply" = 1 ]; then
    plan=$("$wc_bin" disk-cleanup --once --to-target)
  else
    plan=$("$wc_bin" disk-cleanup --once --dry-run)
  fi
  printf 'STADO_RECLAIM_CLEANUP\t%s\t%s\t%s\n' "$before" "$(free_kb)" "$plan"
fi
fi

if stage_enabled build_scratch; then
before=$(free_kb)
if [ -d "$scratch" ]; then
  for entry in "$scratch"/*; do
    [ -e "$entry" ] || continue
    stale "$entry" || continue
    reclaim "$entry" build_scratch
  done
fi
printf 'STADO_RECLAIM_STAGE\tbuild_scratch\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled queue_workdirs; then
before=$(free_kb)
# The queue store is the primary terminality authority. If it is unavailable,
# never turn that absence into an empty keep-list. Instead require three local
# facts together: no process names the tree, the tree has not changed for a
# whole worker-lease window, and that absence was observed continuously for a
# whole lease window. The first apply records the observation; only a later
# pass can reclaim, so elapsed time alone never authorizes deletion.
for workroot in @WORK_ROOTS@; do
  [ -n "$workroot" ] && [ -d "$workroot" ] || continue
  for entry in "$workroot"/wc-* "$workroot"/stado-bootstrap-*; do
    [ -d "$entry" ] || continue
    id=$(basename "$entry")
    case "$id" in
      wc-*) id="${id#wc-}" ;;
      stado-bootstrap-*)
        stale "$entry" || continue
        reclaim "$entry" queue_workdirs
        continue
        ;;
      *) continue ;;
    esac
    if [ "$keep_mode" = store ]; then
      case " @LIVE_JOBS@ " in
        *" $id "*) continue ;;
      esac
      reclaim "$entry" queue_workdirs
    else
      local_evidence "$entry" "$id" || true
    fi
  done
done
printf 'STADO_RECLAIM_STAGE\tqueue_workdirs\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled foreign_home_trees; then
before=$(free_kb)
# macOS-style home trees on a Linux host. `/Users/<name>` exists on Linux only
# as debris of a job or delivery that carried a hard-wired Mac path — on
# 2026-08-19 one such tree held 10.9 GiB of build cache on the GPU builder.
# The uname gate makes this stage a no-op on every macOS host, where /Users is
# the real home root; held() still protects a tree a live process names.
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" = "Linux" ] && [ -d /Users ]; then
  for entry in /Users/*; do
    [ -d "$entry" ] || continue
    reclaim "$entry" foreign_home_trees
  done
fi
printf 'STADO_RECLAIM_STAGE\tforeign_home_trees\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled delivered_trees; then
before=$(free_kb)
# One directory of versions: keep what `current` resolves to, keep the newest,
# take the stale unheld rest. A function because the same rules have to hold
# for the services root and for every superseded delivery root a product
# declares -- two copies would be two policies, and only one of them would be
# the tested one.
sweep_versions() {
  product="$1"
  # What `current` resolves to, in the spelling the version glob produces,
  # so the comparison below is an equality and not a guess.
  keep=""
  if [ -L "$product/current" ]; then
    keep=$(/usr/bin/readlink "$product/current" 2>/dev/null || true)
    case "$keep" in
      "") ;;
      /*) ;;
      *) keep="$product/$keep" ;;
    esac
  fi
  # The newest version directory, `current` itself excluded so a product
  # whose link is stale still keeps its most recent delivery. `ls -td` sorts
  # newest first; the listing is walked with globbing off and IFS on newline
  # so a directory name with a space in it cannot become two words.
  newest=""
  listing=$(/bin/ls -td -- "$product"/*/ 2>/dev/null || true)
  saved_ifs=$IFS
  set -f
  IFS='
'
  for candidate in $listing; do
    candidate=${candidate%/}
    case "$candidate" in
      */current) continue ;;
    esac
    if [ -L "$candidate" ]; then continue; fi
    newest="$candidate"
    break
  done
  IFS=$saved_ifs
  set +f
  # The SAME glob the newest above came from. A `find` here would also list
  # dotted entries the glob cannot see, and the two halves would disagree
  # about the set: on this control plane's own host that difference named
  # `.macos-capability-backup-20260803` -- an operator's state backup living
  # beside the deliveries, which no delivery created and no reclamation may
  # take. A delivery and a build both produce plainly named directories, so
  # the glob IS the set.
  for tree in "$product"/*; do
    [ -d "$tree" ] || continue
    if [ -L "$tree" ]; then continue; fi
    case "$tree" in
      */current) continue ;;
    esac
    [ "$tree" = "$keep" ] && continue
    [ "$tree" = "$newest" ] && continue
    stale "$tree" || continue
    reclaim "$tree" delivered_trees
  done
}

if [ -d "$services" ]; then
  for product in "$services"/*; do
    [ -d "$product" ] || continue
    sweep_versions "$product"
  done
fi
# Where an EARLIER delivery mechanism staged one directory per version, taken
# from what `data/catalog/products.json` declares per product, so a delivery path that
# changes again is a declaration change and not a change here. Each root IS one
# product's version directory, one level shallower than the services layout.
for superseded in @SUPERSEDED_ROOTS@; do
  [ -d "$superseded" ] || continue
  sweep_versions "$superseded"
done
printf 'STADO_RECLAIM_STAGE\tdelivered_trees\t%s\t%s\n' "$before" "$(free_kb)"
fi

"#;

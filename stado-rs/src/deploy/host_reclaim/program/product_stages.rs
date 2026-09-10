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

# One directory of versions: keep what `current` resolves to, keep a pinned
# version, keep the newest, take the stale unheld rest. A function because the
# same rules have to hold for the services root, for every superseded delivery
# root a product declares, and for the delivered-release trees below -- three
# copies would be three policies, and only one of them would be the tested one.
# Defined outside every stage gate so each stage that needs it finds it.
sweep_versions() {
  product="$1"
  # The stage each taken tree is reported under: an item is drained into the
  # stage line that names it, so a tree this function takes for one stage and
  # reports under another is a tree the report attributes to the wrong sweep.
  sweep_stage="$2"
  # Versions the caller pins by name, space-separated, kept beside `current`
  # and the newest. Empty pins nothing.
  pins=" ${3:-} "
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
    case "$pins" in *" ${tree##*/} "*) continue ;; esac
    [ "$tree" = "$newest" ] && continue
    stale "$tree" || continue
    reclaim "$tree" "$sweep_stage"
  done
}

if stage_enabled delivered_trees; then
before=$(free_kb)
if [ -d "$services" ]; then
  for product in "$services"/*; do
    [ -d "$product" ] || continue
    sweep_versions "$product" delivered_trees
  done
fi
# Where an EARLIER delivery mechanism staged one directory per version, taken
# from what `data/products.json` declares per product, so a delivery path that
# changes again is a declaration change and not a change here. Each root IS one
# product's version directory, one level shallower than the services layout.
for superseded in @SUPERSEDED_ROOTS@; do
  [ -d "$superseded" ] || continue
  sweep_versions "$superseded" delivered_trees
done
printf 'STADO_RECLAIM_STAGE\tdelivered_trees\t%s\t%s\n' "$before" "$(free_kb)"
fi

if stage_enabled delivery_leftovers; then
before=$(free_kb)
# What `stado release install-local` leaves behind on every host it delivers
# to, and nothing else reclaims: under `~/.stado/releases/<product>/<version>/`
# the attestation copy and retained archive of each delivered version, and
# under `~/.stado/bin` one dated backup of the binary per install day. Each
# Stado release adds roughly 200 MB of both per host; on 2026-09-10 the Linux
# builder carried 3.0 GiB of the former and 1.35 GiB of the latter, and the
# always-on mini 9.1 GiB, while every declared cleaner reported nothing to
# take.
#
# The version the host is running is pinned by the installed coordinate the
# same delivery writes (`~/.stado/bin/<product>.release-version`): its
# attestation copy is what `stado service converge` byte-compares the
# installed binary against, and deleting it turns a delivered host into an
# unattested one. The newest version stays as the same rules keep for
# services; the stale rest is taken.
releases="$HOME/.stado/releases"
if [ -d "$releases" ]; then
  for product in "$releases"/*; do
    [ -d "$product" ] || continue
    name=${product##*/}
    pins=""
    handshake="$HOME/.stado/bin/$name.release-version"
    if [ -r "$handshake" ]; then
      read -r pins < "$handshake" || pins=""
    fi
    # The version whose attestation copy IS the installed binary, byte for
    # byte, is pinned whether or not a coordinate names it: that copy is the
    # one `stado service converge` attests the host with, and a product
    # delivered by the other delivery path writes no coordinate at all. `cmp`
    # stops at the first differing byte, so every copy but the matching one
    # costs a few kilobytes to rule out.
    if [ -f "$HOME/.stado/bin/$name" ]; then
      for copy in "$product"/*/*/"$name"; do
        [ -f "$copy" ] || continue
        if /usr/bin/cmp -s "$copy" "$HOME/.stado/bin/$name" 2>/dev/null; then
          version=${copy%/*/"$name"}
          pins="$pins ${version##*/}"
        fi
      done
    fi
    sweep_versions "$product" delivery_leftovers "$pins"
  done
fi
# One backup per name survives: the newest, which is the binary the last
# install replaced and the one a rollback by hand would reach for. The rest
# are older binaries with nothing left to roll back to.
bin="$HOME/.stado/bin"
if [ -d "$bin" ]; then
  for newest_backup in "$bin"/*.release-backup-*; do
    [ -f "$newest_backup" ] || continue
    name=${newest_backup##*/}
    name=${name%.release-backup-*}
    # The newest by stamp, once per name: the loop reaches every backup of a
    # name, and every backup of the same name resolves the same newest.
    newest=$(/bin/ls -d -- "$bin/$name".release-backup-* 2>/dev/null | /usr/bin/sort | /usr/bin/tail -n 1)
    [ "$newest_backup" = "$newest" ] && continue
    stale "$newest_backup" || continue
    reclaim "$newest_backup" delivery_leftovers
  done
fi
printf 'STADO_RECLAIM_STAGE\tdelivery_leftovers\t%s\t%s\n' "$before" "$(free_kb)"
fi

"#;

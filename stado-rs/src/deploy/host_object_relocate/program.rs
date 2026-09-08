//! The fixed remote program, and the substitution that turns it into one
//! pass's script.

use crate::deploy::shlex_quote;
use crate::object_store::ROOT_PREFIX;

/// The fixed remote program. [`remote_script`] splices the store root, the
/// key prefixes, the apply flag and the pass bound.
const REMOTE_SCRIPT_TEMPLATE: &str = r#"set -u
root=@ROOT@
# Braced, every one of them: a prefix is spliced immediately after the
# expansion, and `$base` followed by a letter is the variable `baseecosystem`
# as far as the shell is concerned.
base="${root}/@KEYROOT@"
srcpfx="${base}@FROM@"
dstpfx="${base}@TO@"
apply=@APPLY@
limit=@LIMIT@
if [ ! -d "$root" ]; then
  printf 'STADO_RELOCATE_NO_ROOT\t%s\n' "$root"
  exit 0
fi
# One hasher, chosen once. macOS ships shasum, Linux sha256sum, and a host
# with neither must say so rather than move a body it cannot verify.
if [ -x /usr/bin/shasum ]; then
  hash_of() { /usr/bin/shasum -a 256 "$1" 2>/dev/null | /usr/bin/awk '{ print $1 }'; }
elif [ -x /usr/bin/sha256sum ]; then
  hash_of() { /usr/bin/sha256sum "$1" 2>/dev/null | /usr/bin/awk '{ print $1 }'; }
else
  printf 'STADO_RELOCATE_NO_HASHER\t%s\n' "$(/usr/bin/uname -s)"
  exit 0
fi
# `LocalBackend::metadata_path`: the sidecar of a `.json` blob keeps its own
# name, everything else gains the suffix.
meta_of() {
  relative=${1#"$root"/}
  case "$relative" in
    *.json) printf '%s' "$root/.metadata/$relative" ;;
    *) printf '%s' "$root/.metadata/$relative.json" ;;
  esac
}
# The two spellings of the address that appear INSIDE a sidecar, which
# records `stado-uri` as a whole `stado://<namespace>/<key>` string. The
# substitution is prefix-level, so it is one pair of words for the entire
# pass rather than a pair per object.
old_marker="stado://@NAMESPACE@/@FROM@"
new_marker="stado://@NAMESPACE@/@TO@"
# `sed` takes any byte as its delimiter, and a store key may contain every
# character that is not a slash or a newline — including `/`, `|` and `#`.
# A control byte cannot reach here: `validate_prefix` refuses one before this
# program is assembled.
uri_delim=$(printf '\001')
/bin/mkdir -p "$root/.locks" 2>/dev/null
printf 'STADO_RELOCATE_ROOT\t%s\t%s\t%s\n' "$root" "$srcpfx" "$dstpfx"
# The candidate list is taken WHOLE before anything moves, because a
# destination prefix can be an ancestor of the source prefix — which is
# exactly the doubled-namespace case this was written for — and a live walk
# would then meet the objects it had just relocated.
scan=${srcpfx%/*}
scanned=0
decided=0
moved=0
moved_bytes=0
refused=0
if [ -d "$scan" ]; then
  # The list lives in the store's own `.locks/` directory, which
  # `LocalBackend::is_internal` excludes from every listing. A scratch file at
  # the store root would be an object as far as `list` is concerned.
  candidates="$root/.locks/.stado-relocate-candidates.$$"
  /usr/bin/find "$scan" -type f 2>/dev/null | /usr/bin/sort > "$candidates"
  while IFS= read -r source; do
    case "$source" in "$srcpfx"*) ;; *) continue ;; esac
    scanned=$((scanned + 1))
    if [ "$limit" -gt 0 ] && [ "$decided" -ge "$limit" ]; then continue; fi
    relative=${source#"$srcpfx"}
    destination="${dstpfx}${relative}"
    bytes=$(/usr/bin/wc -c < "$source" 2>/dev/null | /usr/bin/tr -d ' ')
    source_key=${source#"$root"/}
    destination_key=${destination#"$root"/}
    if [ "$apply" != yes ]; then
      verdict=would_move
      if [ -e "$destination" ]; then verdict=destination_differs; fi
      decided=$((decided + 1))
      printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
        "$verdict" "$bytes" '-' "$source_key" "$destination_key"
      continue
    fi
    decided=$((decided + 1))
    source_hash=$(hash_of "$source")
    if [ -e "$destination" ]; then
      # Not an error on its own: an interrupted pass leaves precisely this.
      # Equal bytes mean the move happened and only the unlink is owed.
      if [ "$(hash_of "$destination")" = "$source_hash" ] && [ -n "$source_hash" ]; then
        /bin/rm -f "$source"
        source_meta=$(meta_of "$source")
        [ -f "$source_meta" ] && /bin/rm -f "$source_meta"
        moved=$((moved + 1))
        moved_bytes=$((moved_bytes + bytes))
        printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
          'converged' "$bytes" "$source_hash" "$source_key" "$destination_key"
      else
        refused=$((refused + 1))
        printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
          'destination_differs' "$bytes" "$source_hash" "$source_key" "$destination_key"
      fi
      continue
    fi
    /bin/mkdir -p "$(/usr/bin/dirname "$destination")" 2>/dev/null
    # `ln` and not `mv`: it fails on an existing destination instead of
    # clobbering it, and it leaves the source in place to be verified
    # against. Same directory tree, so no bytes are copied.
    if ! /bin/ln "$source" "$destination" 2>/dev/null; then
      refused=$((refused + 1))
      printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
        'link_failed' "$bytes" "$source_hash" "$source_key" "$destination_key"
      continue
    fi
    if [ -z "$source_hash" ] || [ "$(hash_of "$destination")" != "$source_hash" ]; then
      /bin/rm -f "$destination"
      refused=$((refused + 1))
      printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
        'verify_failed' "$bytes" "$source_hash" "$source_key" "$destination_key"
      continue
    fi
    source_meta=$(meta_of "$source")
    destination_meta=$(meta_of "$destination")
    if [ -f "$source_meta" ]; then
      /bin/mkdir -p "$(/usr/bin/dirname "$destination_meta")" 2>/dev/null
      if /bin/ln -f "$source_meta" "$destination_meta" 2>/dev/null; then
        /bin/rm -f "$source_meta"
        printf 'STADO_RELOCATE_META\t%s\t%s\n' 'moved' "$source_key"
      else
        printf 'STADO_RELOCATE_META\t%s\t%s\n' 'link_failed' "$source_key"
      fi
    fi
    # The address recorded INSIDE the sidecar is not corrected here. It is
    # corrected by the reconcile stage below, which is the same substitution
    # over the same two prefixes and also reaches sidecars whose bodies were
    # relocated by something else — the 84 objects a one-off script had
    # already moved when this command was written carried exactly that
    # damage, and a fix that only ran on this pass's own moves would have
    # left them stating the address they no longer have.
    /bin/rm -f "$source"
    moved=$((moved + 1))
    moved_bytes=$((moved_bytes + bytes))
    printf 'STADO_RELOCATE\t%s\t%s\t%s\t%s\t%s\n' \
      'moved' "$bytes" "$source_hash" "$source_key" "$destination_key"
  done < "$candidates"
  /bin/rm -f "$candidates"
  # The emptied directories of the old address. Left behind they are what
  # makes a repaired store still look mis-addressed to anyone reading `du`.
  # Counted as the difference the delete made, not as the empty directories
  # seen before it: `-delete` empties parents as it descends, so the count
  # taken first is a guess and the difference is the measurement.
  pruned=0
  if [ "$apply" = yes ] && [ "$scan" != "${base%/}" ] && [ -d "$scan" ]; then
    before=$(/usr/bin/find "$scan" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
    /usr/bin/find "$scan" -type d -empty -delete 2>/dev/null
    after=$(/usr/bin/find "$scan" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
    pruned=$((before - after))
  fi
  printf 'STADO_RELOCATE_PRUNED\t%s\n' "$pruned"
fi
# Every sidecar under the destination that still records the address the
# objects were moved OFF. `set_metadata` stores `stado-uri` verbatim, so a
# body that arrived by any route other than a fresh PUT keeps the old
# spelling, and `storage ls --long` then describes an address that resolves
# to nothing. Located with one grep for the old prefix rather than by reading
# every sidecar in the namespace: the stale ones are exactly the ones that
# name it.
stale=0
repaired=0
meta_scan="$root/.metadata/${dstpfx#"${root}/"}"
if [ -d "$meta_scan" ]; then
  stale_list="$root/.locks/.stado-relocate-stale.$$"
  /usr/bin/grep -rlF "$old_marker" "$meta_scan" 2>/dev/null | /usr/bin/sort > "$stale_list"
  while IFS= read -r sidecar; do
    [ -f "$sidecar" ] || continue
    stale=$((stale + 1))
    if [ "$apply" != yes ]; then continue; fi
    if /usr/bin/sed "s${uri_delim}${old_marker}${uri_delim}${new_marker}${uri_delim}g" \
        < "$sidecar" > "$sidecar.stado-relocate.$$" 2>/dev/null &&
       /bin/mv -f "$sidecar.stado-relocate.$$" "$sidecar"; then
      repaired=$((repaired + 1))
      printf 'STADO_RELOCATE_META\t%s\t%s\n' 'uri_repaired' "${sidecar#"${root}/"}"
    else
      /bin/rm -f "$sidecar.stado-relocate.$$"
      printf 'STADO_RELOCATE_META\t%s\t%s\n' 'uri_repair_failed' "${sidecar#"${root}/"}"
    fi
  done < "$stale_list"
  /bin/rm -f "$stale_list"
fi
printf 'STADO_RELOCATE_STALE_URI\t%s\t%s\n' "$stale" "$repaired"
# Printed whether or not anything matched, so "nothing to relocate" is told
# apart from a prefix that named a tree the host does not have.
printf 'STADO_RELOCATE_END\t%s\t%s\t%s\t%s\t%s\n' \
  "$scanned" "$decided" "$moved" "$moved_bytes" "$refused"
"#;

/// The remote program with this pass's prefixes and bounds in place.
///
/// `namespace`, `from` and `to` are operator words and every one of them is
/// [`shlex_quote`]d — except that they are spliced INSIDE a double-quoted
/// shell word, so they are quoted by construction here: the caller has
/// already refused anything that is not a key prefix.
pub fn remote_script(
    store_root: &str,
    namespace: &str,
    from: &str,
    to: &str,
    apply: bool,
    limit: usize,
) -> String {
    REMOTE_SCRIPT_TEMPLATE
        .replace("@ROOT@", &shlex_quote(store_root))
        // Trailing slash: the key prefixes an operator names are relative to
        // the namespace root, and without it `ecosystem/probierz` and the
        // first prefix would join into one word.
        .replace("@KEYROOT@", &format!("{ROOT_PREFIX}{namespace}/"))
        .replace("@NAMESPACE@", namespace)
        .replace("@FROM@", from)
        .replace("@TO@", to)
        .replace("@APPLY@", if apply { "yes" } else { "no" })
        .replace("@LIMIT@", &limit.to_string())
}

//! `stado host object-relocate TARGET` — move objects from one key prefix to
//! another INSIDE the store, on the host that holds it.
//!
//! The object API exposes GET, PUT, DELETE, list and stat and nothing else:
//! there is no move and no server-side copy. So for as long as this fleet has
//! needed to re-address an object, the only way to do it was to download the
//! body to the control plane and upload it back under the other key. On
//! 2026-08-30 that is what took the always-on mac's release ingress down for
//! ten minutes: 134 MiB GGUF parts of `jeden-goal-qwen3-4b` pulled through the
//! loopback writer tunnel, retried, and a peer's publish answering 502 behind
//! them. The bytes never needed to move at all. Both stores are directories on
//! that machine, and a re-address inside one directory tree is a `link` and an
//! `unlink` — no network, no writer, no body in flight.
//!
//! Adding a move route to the object API would have been the other answer. It
//! is the worse one: a new verb on a live store, reachable by anything holding
//! a bearer, to serve a defect's cleanup. This command needs the same host
//! shell the read-only `host disk` already speaks, so it uses it.
//!
//! The shape and the rules come from [`crate::deploy::host_disk`] via
//! [`crate::deploy::host_channel`]: one FIXED remote program, registry data
//! reaching ssh only as the destination, every value spliced through
//! [`shlex_quote`], the tab-delimited `STADO_*` marker protocol on the way
//! back, and the report closed by [`host_channel::finish_report`].
//!
//! What the remote program guarantees, because a relocation that loses an
//! object is worse than the mis-addressing it repairs:
//!
//! - **It refuses to overwrite.** A destination that already exists is never
//!   written. When its content hashes equal the source's, the pair is a
//!   half-finished earlier move and the source is dropped; when they differ,
//!   the object is reported [`DESTINATION_DIFFERS`] and BOTH copies are kept.
//! - **It verifies before it removes.** The destination is hard-linked into
//!   place, hashed from disk, and compared against the source's hash. Only
//!   then is the source unlinked. A mismatch unlinks the destination and keeps
//!   the source.
//! - **It is resumable.** Every outcome is a function of what is on disk, so a
//!   run that was interrupted, timed out, or bounded by `--limit` is finished
//!   by running it again. Nothing is recorded anywhere but the store itself.
//! - **It previews by default.** `--apply` is the only thing that changes a
//!   byte, the same way [`crate::deploy::host_reclaim`] is written.
//!
//! The metadata sidecar travels with the body. `LocalBackend` keeps it at
//! `.metadata/<path>.json` beside the store root and `delete` removes both, so
//! a body moved without its sidecar would leave `list --long` describing the
//! old address and the new object carrying nothing.
//!
//! Like [`crate::deploy::host_disk`]'s script the remote program is a raw
//! string: `\t` / `\n` inside it are the two literal characters the remote
//! `printf` expands, not Rust escapes.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::host_channel;
use super::{shlex_quote, DeployError, Runner};
use crate::object_store::ROOT_PREFIX;
use crate::targets::ComputeTarget;

/// `status` for a run whose every object reached a decided outcome.
pub const OK_STATUS: &str = "ok";

/// The store root, relative to the remote login user's `$HOME`, when the
/// operator names none.
///
/// The same default `storage.local.path` carries
/// ([`crate::config::wc_local_storage_path`]), written relative because
/// `$HOME` expands only on the far side. It is the object API's own backing
/// directory on the always-on mac.
pub const DEFAULT_STORE_ROOT: &str = ".stado/local-storage";

/// Wall clock for one pass.
///
/// Deliberately not [`host_channel::remote_timeout`]'s two minutes: this pass
/// hashes every body it moves twice, and the objects that made the command
/// necessary are 134 MiB each. Half an hour covers the whole nested tree in
/// one call; `--limit` bounds a pass that has to be shorter.
pub const TIMEOUT_SECONDS: u64 = 1800;

/// Moved: linked, hashed, compared, source unlinked.
pub const MOVED: &str = "moved";
/// `--apply` was not given, and this object is what a pass would move.
pub const WOULD_MOVE: &str = "would_move";
/// The destination already held these exact bytes, so an earlier pass had
/// linked it and died before unlinking the source. The source is gone now.
pub const CONVERGED: &str = "converged";
/// The destination exists with OTHER content. Both copies kept, untouched.
pub const DESTINATION_DIFFERS: &str = "destination_differs";
/// The link was made and the destination did not hash to the source. The
/// destination was unlinked; the source is untouched.
pub const VERIFY_FAILED: &str = "verify_failed";
/// The host refused the link itself. Nothing changed.
pub const LINK_FAILED: &str = "link_failed";

/// Every outcome that leaves the object where it was found, in need of an
/// operator: the two `--json` consumers and the printer agree on one list
/// rather than each spelling its own.
pub fn is_refusal(outcome: &str) -> bool {
    matches!(outcome, DESTINATION_DIFFERS | VERIFY_FAILED | LINK_FAILED)
}

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

/// One object the pass reached a verdict about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relocation {
    pub outcome: String,
    pub bytes: i64,
    pub sha256: Option<String>,
    pub source_key: String,
    pub destination_key: String,
}

/// One sidecar that travelled, or refused to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataMove {
    pub outcome: String,
    pub source_key: String,
}

/// Everything one pass answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocateReading {
    pub store_root: Option<String>,
    pub source_prefix: Option<String>,
    pub destination_prefix: Option<String>,
    /// The store root is not a directory on this host, so nothing was read.
    pub missing_root: Option<String>,
    /// No sha256 program, so nothing was moved: a body this command cannot
    /// verify is a body it does not touch.
    pub no_hasher: Option<String>,
    pub objects: Vec<Relocation>,
    pub metadata: Vec<MetadataMove>,
    pub scanned: i64,
    pub decided: i64,
    pub moved: i64,
    pub moved_bytes: i64,
    pub refused: i64,
    pub pruned_directories: i64,
    /// Sidecars under the destination whose recorded `stado-uri` still names
    /// the source prefix, and how many of those this pass rewrote.
    pub stale_uris: i64,
    pub repaired_uris: i64,
    /// The closing marker arrived, so the totals are the host's own and not a
    /// truncated read.
    pub complete: bool,
}

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str) -> RelocateReading {
    let mut reading = RelocateReading::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_RELOCATE_NO_ROOT", root] => reading.missing_root = Some((*root).to_string()),
            ["STADO_RELOCATE_NO_HASHER", os] => reading.no_hasher = Some((*os).to_string()),
            ["STADO_RELOCATE_ROOT", root, source, destination] => {
                reading.store_root = Some((*root).to_string());
                reading.source_prefix = Some((*source).to_string());
                reading.destination_prefix = Some((*destination).to_string());
            }
            ["STADO_RELOCATE", outcome, bytes, sha, source, destination] => {
                reading.objects.push(Relocation {
                    outcome: (*outcome).to_string(),
                    bytes: bytes.parse::<i64>().unwrap_or_default(),
                    sha256: match *sha {
                        "-" | "" => None,
                        value => Some(value.to_string()),
                    },
                    source_key: (*source).to_string(),
                    destination_key: (*destination).to_string(),
                });
            }
            ["STADO_RELOCATE_META", outcome, source] => {
                reading.metadata.push(MetadataMove {
                    outcome: (*outcome).to_string(),
                    source_key: (*source).to_string(),
                });
            }
            ["STADO_RELOCATE_PRUNED", count] => {
                reading.pruned_directories = count.parse::<i64>().unwrap_or_default();
            }
            ["STADO_RELOCATE_STALE_URI", stale, repaired] => {
                reading.stale_uris = stale.parse::<i64>().unwrap_or_default();
                reading.repaired_uris = repaired.parse::<i64>().unwrap_or_default();
            }
            ["STADO_RELOCATE_END", scanned, decided, moved, moved_bytes, refused] => {
                reading.scanned = scanned.parse::<i64>().unwrap_or_default();
                reading.decided = decided.parse::<i64>().unwrap_or_default();
                reading.moved = moved.parse::<i64>().unwrap_or_default();
                reading.moved_bytes = moved_bytes.parse::<i64>().unwrap_or_default();
                reading.refused = refused.parse::<i64>().unwrap_or_default();
                reading.complete = true;
            }
            _ => {}
        }
    }
    reading
}

/// The reading as the `--json` report, in `host disk`'s report shape.
pub fn to_report(
    target: &ComputeTarget,
    reading: &RelocateReading,
    namespace: &str,
    applied: bool,
) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert("namespace".to_string(), json!(namespace));
    report.insert("applied".to_string(), json!(applied));
    report.insert(
        "store".to_string(),
        json!({
            "root": reading.store_root,
            "source_prefix": reading.source_prefix,
            "destination_prefix": reading.destination_prefix,
            "missing_root": reading.missing_root,
            "no_hasher": reading.no_hasher,
        }),
    );
    report.insert(
        "totals".to_string(),
        json!({
            "scanned": reading.scanned,
            "decided": reading.decided,
            "moved": reading.moved,
            "moved_bytes": reading.moved_bytes,
            "refused": reading.refused,
            "pruned_directories": reading.pruned_directories,
            "stale_uris": reading.stale_uris,
            "repaired_uris": reading.repaired_uris,
            // A pass whose closing marker never arrived states so, because the
            // totals of a truncated read are a lower bound and reading them as
            // the answer is how a half-finished relocation looks finished.
            "complete": reading.complete,
            "remaining": (reading.scanned - reading.decided).max(0),
        }),
    );
    report.insert(
        "objects".to_string(),
        Value::Array(
            reading
                .objects
                .iter()
                .map(|item| {
                    json!({
                        "outcome": item.outcome,
                        "bytes": item.bytes,
                        "sha256": item.sha256,
                        "source_key": item.source_key,
                        "destination_key": item.destination_key,
                    })
                })
                .collect(),
        ),
    );
    report.insert(
        "metadata".to_string(),
        Value::Array(
            reading
                .metadata
                .iter()
                .map(|item| {
                    json!({
                        "outcome": item.outcome,
                        "source_key": item.source_key,
                    })
                })
                .collect(),
        ),
    );
    report
}

/// A key prefix an operator may name, or the reason it is refused.
///
/// The prefixes are spliced into the remote program inside a double-quoted
/// word, so what has to be excluded is anything the shell would still read
/// there — `"`, `$`, `` ` `` and `\` — plus the traversal a store path may
/// never contain. An absolute prefix is refused too: these are keys under a
/// namespace, and one starting with `/` would address the filesystem root.
pub fn validate_prefix(label: &str, prefix: &str) -> Result<(), DeployError> {
    if prefix.starts_with('/') {
        return Err(DeployError(format!(
            "{label} must be a key prefix inside the namespace, not an absolute path: {prefix}"
        )));
    }
    if prefix.split('/').any(|segment| segment == "..") {
        return Err(DeployError(format!(
            "{label} may not contain a `..` segment: {prefix}"
        )));
    }
    if let Some(bad) = prefix.chars().find(|character| {
        matches!(character, '"' | '$' | '`' | '\\' | '\'' | '\t' | '\n') || character.is_control()
    }) {
        return Err(DeployError(format!(
            "{label} may not contain {bad:?}: {prefix}"
        )));
    }
    Ok(())
}

/// One relocation pass, as the operator named it.
///
/// A struct and not six arguments because the CLI hands the same six words
/// through, and a `bool` pair plus three prefixes in positional form is how a
/// destination and a source end up swapped by a caller reading the wrong
/// line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocatePlan {
    /// Store namespace holding both addresses.
    pub namespace: String,
    /// The mis-addressed key prefix.
    pub from: String,
    /// The key prefix the objects belong under; empty is the namespace root.
    pub to: String,
    /// Store root on the host, or [`DEFAULT_STORE_ROOT`] under its `$HOME`.
    pub store_root: Option<String>,
    /// Change bytes. False previews.
    pub apply: bool,
    /// Decide at most this many objects; 0 is all of them.
    pub limit: usize,
}

/// Relocate one key prefix to another inside one canonical registry host's
/// store, or report what a pass would move.
pub async fn relocate_host(
    target_name: &str,
    plan: &RelocatePlan,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let RelocatePlan {
        namespace,
        from,
        to,
        store_root,
        apply,
        limit,
    } = plan;
    let (apply, limit) = (*apply, *limit);
    validate_prefix("--namespace", namespace)?;
    if namespace.is_empty() || namespace.contains('/') {
        return Err(DeployError(format!(
            "--namespace is one store namespace, e.g. probierz: {namespace}"
        )));
    }
    validate_prefix("--from-prefix", from)?;
    validate_prefix("--to-prefix", to)?;
    if from == to {
        return Err(DeployError(format!(
            "--from-prefix and --to-prefix name the same address, so there is nothing to move: {from}"
        )));
    }
    if from.is_empty() {
        return Err(DeployError(
            "--from-prefix would select the whole namespace; name the mis-addressed prefix"
                .to_string(),
        ));
    }
    let target = host_channel::canonical_target(target_name).await?;
    // `$HOME` expands only on the far side, so an unnamed root is composed
    // there rather than guessed from this machine's own configuration.
    let root = match store_root {
        Some(named) => named.to_string(),
        None => format!(
            "{}/{DEFAULT_STORE_ROOT}",
            host_channel::remote_home(&target, runner).await?
        ),
    };
    let script = remote_script(&root, namespace, from, to, apply, limit);
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(TIMEOUT_SECONDS),
        runner,
    )
    .await?;
    let reading = parse_output(&output.stdout);
    let mut report = to_report(&target, &reading, namespace, apply);
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}

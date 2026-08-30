//! Classify a host's local disaster-recovery replica against the store it is
//! supposed to mirror, object by object, before anything is deleted.
//!
//! Written for charless-mac-mini on 2026-08-30, where the replica had become
//! the largest single consumer on the one machine whose disk was blocking a
//! stalled queue: 48.5 GiB of `~/.stado/local-backup` against a 32.7 GiB
//! primary. It got there because replication crossed two addressings — the
//! object API answers in bare ecosystem keys, a directory in
//! namespace-qualified store paths — so every pass wrote its objects at names
//! nothing resolves, and [`crate::queue::copy::prune_backup_extras`] only ever
//! swept the canonical prefixes, leaving the rest to accumulate for the lifetime
//! of the host.
//!
//! Reclaiming it needs a number nobody had: how much of that replica exists,
//! intact, in the primary. The first attempt at a similar question — the
//! doubly-nested tree in the primary — was assumed to be duplicate data and
//! turned out to be the ONLY copy of 9.58 GiB of trained-model artifacts, which
//! is the whole reason this command reports a classification rather than
//! deleting anything. Nothing here removes a byte.
//!
//! **Everything runs on the host, and no object body crosses the network.** Both
//! stores are directories on that machine — the API's own backing store and the
//! replica beside it — so the comparison is local file work. That matters beyond
//! tidiness: pulling 134 MiB bodies through the control plane's loopback writer
//! is what took that host's release ingress down earlier the same day.
//!
//! Classification, per file in the replica:
//!
//! - **`twin`** — the primary holds the same address with the same size AND the
//!   same SHA-256. Only these are safe to drop.
//! - **`differs`** — the primary holds that address with different content. Data,
//!   and possibly the newer of the two. Kept.
//! - **`absent`** — the primary does not hold that address at all. This is the
//!   sole-copy case, and on this host it is the expected verdict for everything
//!   the mis-addressed replication wrote. Kept.
//!
//! Hashing is the expensive half, so it runs only where a size match already
//! makes a twin possible; `absent` and a size mismatch are decided without
//! reading a byte.
//!
//! The address mapping is the same rule the copier now refuses to cross. A
//! replica path already under `ecosystem/` is a qualified store path and maps
//! straight through; a bare path is what a cross-addressed pass wrote, and its
//! primary address is that path inside the configured namespace.

use std::collections::BTreeMap;

use crate::targets::ComputeTarget;

use super::host_channel;
use super::{DeployError, Runner};

/// Marker for the namespace the bare-path arm maps into.
const NAMESPACE_MARK: &str = "@NAMESPACE@";
/// Marker for the replica root, relative to the remote home.
const BACKUP_ROOT_MARK: &str = "@BACKUP_ROOT@";
/// Marker for the primary store root, relative to the remote home.
const PRIMARY_ROOT_MARK: &str = "@PRIMARY_ROOT@";

/// Proven present in the primary with identical bytes. Only these are safe to
/// drop.
pub const TWIN: &str = "twin";
/// The primary has this address with different content. Data; kept.
pub const DIFFERS: &str = "differs";
/// The primary does not have this address at all. The sole-copy case; kept.
pub const ABSENT: &str = "absent";
/// The primary has this address at the same size, but the pass ran out of its
/// hashing budget before proving the bytes match.
///
/// Reported as its own class rather than folded into [`TWIN`], because the one
/// thing this command exists to prevent is treating an unproven twin as
/// reclaimable. A size match is not identity.
pub const SAME_SIZE_UNPROVEN: &str = "same_size_unproven";

/// How long the remote program may spend hashing, in seconds.
///
/// The fleet channel gives every script 120 seconds, and this replica is
/// 48.5 GiB — far more than `shasum` can read in that window on a host that is
/// also running jobs. So the size comparison, which is one `stat` per file and
/// decides [`ABSENT`] and most of [`DIFFERS`] outright, always completes; the
/// hashing that proves a twin runs until this deadline and then stops, leaving
/// the rest honestly labelled. Repeated runs make more of it provable as the
/// twin set shrinks under whatever the operator then reclaims.
const HASH_DEADLINE_SECONDS: u64 = 70;

/// The fixed remote program. Read-only: it walks the replica once, compares
/// sizes, and hashes same-size pairs until its deadline.
///
/// One `python3` process rather than shell with per-file `stat`: the replica
/// holds tens of thousands of objects, and a fork or three for each of them
/// does not finish inside the fleet channel's 120-second budget. The first
/// version of this script did exactly that and timed out twice before reaching
/// a single hash. The roots arrive through the environment so nothing operator-
/// supplied is ever spliced into a program text.
const REMOTE_SCRIPT_TEMPLATE: &str = r#"set -u
backup="$HOME/@BACKUP_ROOT@"
primary="$HOME/@PRIMARY_ROOT@"
if [ ! -d "$backup" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'replica root is absent'
  exit 0
fi
if [ ! -d "$primary" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'primary store root is absent'
  exit 0
fi
STADO_BACKUP_ROOT="$backup" \
STADO_PRIMARY_ROOT="$primary" \
STADO_NAMESPACE='@NAMESPACE@' \
STADO_HASH_DEADLINE='@HASH_DEADLINE@' \
/usr/bin/python3 - <<'STADO_AUDIT_EOF'
import hashlib, os, sys, time
backup = os.environ["STADO_BACKUP_ROOT"]
primary = os.environ["STADO_PRIMARY_ROOT"]
namespace = os.environ["STADO_NAMESPACE"]
deadline = time.monotonic() + float(os.environ["STADO_HASH_DEADLINE"])

def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()

out = sys.stdout
for root, _, files in os.walk(backup):
    for name in files:
        path = os.path.join(root, name)
        relative = os.path.relpath(path, backup)
        if relative.startswith("ecosystem/"):
            candidate = os.path.join(primary, relative)
        else:
            candidate = os.path.join(primary, "ecosystem", namespace, relative)
        try:
            size = os.lstat(path).st_size
        except OSError:
            continue
        try:
            other = os.lstat(candidate).st_size
        except OSError:
            out.write("STADO_BACKUP_AUDIT\tabsent\t%d\t%s\n" % (size, relative))
            continue
        if other != size:
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        if time.monotonic() >= deadline:
            out.write("STADO_BACKUP_AUDIT\tsame_size_unproven\t%d\t%s\n" % (size, relative))
            continue
        try:
            same = digest(path) == digest(candidate)
        except OSError:
            out.write("STADO_BACKUP_AUDIT\tsame_size_unproven\t%d\t%s\n" % (size, relative))
            continue
        out.write(
            "STADO_BACKUP_AUDIT\t%s\t%d\t%s\n"
            % ("twin" if same else "differs", size, relative)
        )
out.write("STADO_BACKUP_AUDIT_END\tclassified\n")
STADO_AUDIT_EOF
"#;

/// The remote program with this host's roots, namespace and hashing deadline in
/// place.
pub fn remote_script(namespace: &str, backup_root: &str, primary_root: &str) -> String {
    REMOTE_SCRIPT_TEMPLATE
        .replace(NAMESPACE_MARK, namespace)
        .replace(BACKUP_ROOT_MARK, backup_root)
        .replace(PRIMARY_ROOT_MARK, primary_root)
        .replace("@HASH_DEADLINE@", &HASH_DEADLINE_SECONDS.to_string())
}

/// One class's totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassTotals {
    pub objects: u64,
    pub bytes: u64,
}

/// The whole reading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupAudit {
    pub host: String,
    /// Per-class totals, keyed by [`TWIN`], [`DIFFERS`], [`ABSENT`].
    pub classes: BTreeMap<String, ClassTotals>,
    /// The largest few of each class, for a report that names things rather
    /// than only counting them.
    pub examples: BTreeMap<String, Vec<(u64, String)>>,
    /// Set when the host could not be classified at all.
    pub unavailable: Option<String>,
    /// True once the remote program printed its end marker, so a truncated
    /// channel is never read as "nothing to reclaim".
    pub complete: bool,
}

impl BackupAudit {
    /// Bytes that are proven present and intact in the primary.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.classes.get(TWIN).map(|t| t.bytes).unwrap_or_default()
    }

    /// Bytes that are data: the primary either lacks them or holds something
    /// else at that address.
    pub fn retained_bytes(&self) -> u64 {
        [DIFFERS, ABSENT, SAME_SIZE_UNPROVEN]
            .iter()
            .filter_map(|class| self.classes.get(*class))
            .map(|totals| totals.bytes)
            .sum()
    }
}

/// Parse the remote program's output.
pub fn parse_output(stdout: &str, host: &str) -> BackupAudit {
    let mut audit = BackupAudit {
        host: host.to_string(),
        ..BackupAudit::default()
    };
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("STADO_BACKUP_AUDIT") => {
                let (Some(class), Some(size), Some(path)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                if !matches!(class, TWIN | DIFFERS | ABSENT | SAME_SIZE_UNPROVEN) {
                    continue;
                }
                let bytes = size.trim().parse::<u64>().unwrap_or_default();
                let totals = audit.classes.entry(class.to_string()).or_default();
                totals.objects += 1;
                totals.bytes += bytes;
                let examples = audit.examples.entry(class.to_string()).or_default();
                examples.push((bytes, path.to_string()));
                examples.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
                examples.truncate(5);
            }
            Some("STADO_BACKUP_AUDIT_UNAVAILABLE") => {
                audit.unavailable = fields.next().map(str::to_string);
            }
            Some("STADO_BACKUP_AUDIT_END") => audit.complete = true,
            _ => {}
        }
    }
    audit
}

/// Classify `host`'s replica against its primary store.
///
/// Read-only end to end: the remote program stats and hashes, and this function
/// only counts what it printed.
pub async fn audit_host(
    host: &str,
    namespace: &str,
    backup_root: &str,
    primary_root: &str,
    runner: &Runner,
) -> Result<(ComputeTarget, BackupAudit), DeployError> {
    let target = host_channel::canonical_target(host).await?;
    let script = remote_script(namespace, backup_root, primary_root);
    let output = host_channel::run_script(&target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not classify its replica",
        )));
    }
    let audit = parse_output(&output.stdout, &target.name);
    Ok((target, audit))
}

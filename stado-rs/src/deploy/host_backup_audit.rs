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

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::targets::ComputeTarget;

use super::host_channel;
use super::{DeployError, Runner};

/// Marker for the namespace the bare-path arm maps into.
const NAMESPACE_MARK: &str = "@NAMESPACE@";
/// Marker for the replica root, relative to the remote home.
const BACKUP_ROOT_MARK: &str = "@BACKUP_ROOT@";
/// Marker for the primary store root, relative to the remote home.
const PRIMARY_ROOT_MARK: &str = "@PRIMARY_ROOT@";
/// Marker for exact qualified object paths, encoded as comma-separated hex.
const OBJECTS_HEX_MARK: &str = "@OBJECTS_HEX@";
/// Marker for namespace names whose backup-visible object metadata is listed.
const INVENTORY_NAMESPACES_HEX_MARK: &str = "@INVENTORY_NAMESPACES_HEX@";

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

/// How long a READ-ONLY pass may spend hashing, in seconds.
///
/// The fleet channel gives every script 120 seconds, and this replica is
/// 48.5 GiB — far more than `shasum` can read in that window on a host that is
/// also running jobs. So the size comparison, which is one `stat` per file and
/// decides [`ABSENT`] and most of [`DIFFERS`] outright, always completes; the
/// hashing that proves a twin runs until this deadline and then stops, leaving
/// the rest honestly labelled. Repeated runs make more of it provable as the
/// twin set shrinks under whatever the operator then reclaims.
const HASH_DEADLINE_SECONDS: u64 = 70;

/// How long a RECLAIM pass may spend hashing, and how long the channel waits
/// for it.
///
/// A reclaim proves every object it deletes inside the same pass, so it has to
/// read both copies of everything it intends to drop — twice 38.47 GiB on this
/// host. Under the read-only budget it would prove almost nothing and delete
/// almost nothing, and the temptation would then be to delete against the
/// previous run's recorded verdict, which is exactly the mistake that turns a
/// replica into data loss. So the pass gets a budget that fits the work.
const RECLAIM_HASH_DEADLINE_SECONDS: u64 = 1500;
/// Wall clock for a reclaim pass on the channel, above its hashing deadline so
/// the program's own deadline is what stops it and the totals still come back.
const RECLAIM_TIMEOUT_SECONDS: u64 = 1800;

/// The fixed remote program. It walks the replica once, compares sizes, hashes
/// same-size pairs until its deadline, and — only under `@RECLAIM@` with
/// `@APPLY@` — unlinks the ones it has just proven identical.
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
# Free space as the host itself measures it, on both sides of the pass. The
# whole point of a reclaim is this number, so it is read by the program that
# changed it rather than by a second command an operator runs afterwards
# against a disk the fleet is still writing to.
free_kb() {
  /bin/df -Pk "$HOME" 2>/dev/null | /usr/bin/awk 'NR == 2 { print $4 }'
}
printf 'STADO_BACKUP_FREE\t%s\t%s\n' 'before' "$(free_kb)"
STADO_BACKUP_ROOT="$backup" \
STADO_PRIMARY_ROOT="$primary" \
STADO_NAMESPACE='@NAMESPACE@' \
STADO_HASH_DEADLINE='@HASH_DEADLINE@' \
STADO_RECLAIM='@RECLAIM@' \
STADO_APPLY='@APPLY@' \
STADO_OBJECTS_HEX='@OBJECTS_HEX@' \
STADO_INVENTORY_NAMESPACES_HEX='@INVENTORY_NAMESPACES_HEX@' \
/usr/bin/python3 - <<'STADO_AUDIT_EOF'
import hashlib, os, stat, sys, time
backup = os.environ["STADO_BACKUP_ROOT"]
primary = os.environ["STADO_PRIMARY_ROOT"]
namespace = os.environ["STADO_NAMESPACE"]
deadline = time.monotonic() + float(os.environ["STADO_HASH_DEADLINE"])
reclaim = os.environ["STADO_RECLAIM"] == "yes"
apply = os.environ["STADO_APPLY"] == "yes"
selected = [
    bytes.fromhex(value).decode("utf-8")
    for value in os.environ["STADO_OBJECTS_HEX"].split(",")
    if value
]
inventory_namespaces = [
    bytes.fromhex(value).decode("utf-8")
    for value in os.environ["STADO_INVENTORY_NAMESPACES_HEX"].split(",")
    if value
]



def digest(path, stop_at=None):
    h = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            if stop_at is not None and time.monotonic() >= stop_at:
                return None
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return h.hexdigest()
            h.update(chunk)

out = sys.stdout
def identity(path):
    try:
        entry = os.lstat(path)
    except FileNotFoundError:
        return ("absent", "", "")
    except OSError:
        return ("unreadable", "", "")
    if not stat.S_ISREG(entry.st_mode):
        return ("not_regular", str(entry.st_size), "")
    try:
        value = digest(path, deadline)
        if value is None:
            return ("deadline_unproven", str(entry.st_size), "")
        return ("present", str(entry.st_size), value)
    except OSError:
        return ("unreadable", str(entry.st_size), "")

def emit_namespaces(label, root):
    ecosystem = os.path.join(root, "ecosystem")
    names = []
    if time.monotonic() >= deadline:
        out.write("STADO_BACKUP_NAMESPACES_ERROR\t%s\tdeadline exhausted\n" % label)
        return
    try:
        with os.scandir(ecosystem) as entries:
            names = sorted(
                entry.name
                for entry in entries
                if entry.is_dir(follow_symlinks=False)
            )
    except OSError as error:
        out.write(
            "STADO_BACKUP_NAMESPACES_ERROR\t%s\t%s\n"
            % (label, str(error).replace("\t", " ").replace("\n", " "))
        )
        return
    if time.monotonic() >= deadline:
        out.write("STADO_BACKUP_NAMESPACES_ERROR\t%s\tdeadline exhausted\n" % label)
        return
    for name in names:
        out.write(
            "STADO_BACKUP_NAMESPACE\t%s\t%s\n"
            % (label, name.encode("utf-8").hex())
        )
    out.write("STADO_BACKUP_NAMESPACES_END\t%s\t%d\n" % (label, len(names)))

emit_namespaces("local_storage", primary)
emit_namespaces("local_backup", backup)

def metadata_path(root, relative):
    name = relative if relative.endswith(".json") else relative + ".json"
    return os.path.join(root, ".metadata", name)

def stat_identity(path):
    try:
        entry = os.lstat(path)
    except FileNotFoundError:
        return ("absent", "", "")
    except OSError:
        return ("unreadable", "", "")
    if not stat.S_ISREG(entry.st_mode):
        return ("not_regular", str(entry.st_size), "")
    return ("present", str(entry.st_size), "")

inventory_complete = True
def inventory_error(scope, detail):
    global inventory_complete
    inventory_complete = False
    out.write(
        "STADO_BACKUP_AUDIT_UNAVAILABLE\t%s inventory: %s\n"
        % (scope, str(detail).replace("\t", " ").replace("\n", " "))
    )

inventory_timed_out = False
for scope in inventory_namespaces:
    if time.monotonic() >= deadline:
        inventory_error(scope, "deadline exhausted before namespace enumeration")
        inventory_timed_out = True
        break
    backup_scope = os.path.join(backup, "ecosystem", scope)
    try:
        scope_entry = os.lstat(backup_scope)
    except OSError as error:
        inventory_error(scope, error)
        continue
    if not stat.S_ISDIR(scope_entry.st_mode):
        inventory_error(scope, "backup namespace root is not a directory")
        continue
    walk_errors = []
    def walk_error(error):
        walk_errors.append(error)
    for root, dirs, files in os.walk(
        backup_scope,
        followlinks=False,
        onerror=walk_error,
    ):
        if time.monotonic() >= deadline:
            inventory_error(scope, "deadline exhausted during namespace enumeration")
            inventory_timed_out = True
            break
        retained_dirs = []
        for name in sorted(dirs):
            directory = os.path.join(root, name)
            if os.path.islink(directory):
                inventory_error(scope, "non-directory entry omitted: " + directory)
            else:
                retained_dirs.append(name)
        dirs[:] = retained_dirs
        for name in sorted(files):
            if time.monotonic() >= deadline:
                inventory_error(scope, "deadline exhausted during namespace enumeration")
                inventory_timed_out = True
                break
            path = os.path.join(root, name)
            relative = os.path.relpath(path, backup)
            b_state, b_size, _ = stat_identity(path)
            p_state, p_size, _ = stat_identity(os.path.join(primary, relative))
            p_meta_state, p_meta_size, _ = stat_identity(metadata_path(primary, relative))
            b_meta_state, b_meta_size, _ = stat_identity(metadata_path(backup, relative))
            out.write(
                "STADO_BACKUP_INVENTORY_OBJECT\t%s\t%s\t%s\t\t%s\t%s\t\t%s\t%s\t\t%s\t%s\t\n"
                % (
                    relative.encode("utf-8").hex(),
                    p_state,
                    p_size,
                    b_state,
                    b_size,
                    p_meta_state,
                    p_meta_size,
                    b_meta_state,
                    b_meta_size,
                )
            )
            if b_state != "present":
                inventory_error(scope, "backup object is " + b_state + ": " + relative)
            for label, state in (
                ("local-storage object", p_state),
                ("local-storage metadata", p_meta_state),
                ("local-backup metadata", b_meta_state),
            ):
                if state == "unreadable":
                    inventory_error(scope, label + " is unreadable: " + relative)
        if inventory_timed_out:
            break
    for error in walk_errors:
        inventory_error(scope, error)
    if inventory_timed_out:
        break

if inventory_namespaces and not selected:
    if inventory_complete:
        out.write("STADO_BACKUP_AUDIT_END\tinventory\n")
    sys.exit(0)


if selected:
    for relative in selected:
        normalized = os.path.normpath(relative)
        if os.path.isabs(relative) or normalized != relative or normalized.startswith("../"):
            out.write("STADO_BACKUP_AUDIT_UNAVAILABLE\tinvalid exact object path\n")
            continue
        p_state, p_size, p_digest = identity(os.path.join(primary, relative))
        b_state, b_size, b_digest = identity(os.path.join(backup, relative))
        p_meta_state, p_meta_size, p_meta_digest = identity(metadata_path(primary, relative))
        b_meta_state, b_meta_size, b_meta_digest = identity(metadata_path(backup, relative))
        out.write(
            "STADO_BACKUP_OBJECT\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n"
            % (
                relative,
                p_state,
                p_size,
                p_digest,
                b_state,
                b_size,
                b_digest,
                p_meta_state,
                p_meta_size,
                p_meta_digest,
                b_meta_state,
                b_meta_size,
                b_meta_digest,
            )
        )
    if inventory_complete:
        out.write("STADO_BACKUP_AUDIT_END\texact\n")
    sys.exit(0)
deleted = 0
deleted_bytes = 0
refused = 0
for root, _, files in os.walk(backup):
    for name in files:
        path = os.path.join(root, name)
        relative = os.path.relpath(path, backup)
        if relative.startswith("ecosystem/"):
            candidate = os.path.join(primary, relative)
        else:
            candidate = os.path.join(primary, "ecosystem", namespace, relative)
        try:
            entry = os.lstat(path)
        except OSError:
            continue
        size = entry.st_size
        try:
            other = os.lstat(candidate)
        except OSError:
            out.write("STADO_BACKUP_AUDIT\tabsent\t%d\t%s\n" % (size, relative))
            continue
        # Only a plain file on BOTH sides can be a twin. A symlink, a socket or
        # a directory that happens to match a size is not the object, and the
        # one thing this pass may never do is unlink something whose primary
        # counterpart it did not actually read.
        if not stat.S_ISREG(entry.st_mode) or not stat.S_ISREG(other.st_mode):
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        if other.st_size != size:
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
        if not same:
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        out.write("STADO_BACKUP_AUDIT\ttwin\t%d\t%s\n" % (size, relative))
        # The proof and the deletion are the same event. Nothing here reads a
        # verdict recorded by an earlier run: the two hashes above were computed
        # from these two files moments ago, and only that proves this unlink.
        if not reclaim:
            continue
        if not apply:
            out.write("STADO_BACKUP_RECLAIM\twould_delete\t%d\t%s\n" % (size, relative))
            continue
        try:
            os.remove(path)
        except OSError:
            refused += 1
            out.write("STADO_BACKUP_RECLAIM\tdelete_failed\t%d\t%s\n" % (size, relative))
            continue
        deleted += 1
        deleted_bytes += size
        out.write("STADO_BACKUP_RECLAIM\tdeleted\t%d\t%s\n" % (size, relative))
out.write(
    "STADO_BACKUP_RECLAIM_END\t%d\t%d\t%d\n" % (deleted, deleted_bytes, refused)
)
out.write("STADO_BACKUP_AUDIT_END\tclassified\n")
STADO_AUDIT_EOF
pruned=0
if [ '@APPLY@' = yes ] && [ '@RECLAIM@' = yes ]; then
  # Counted as the difference the delete made rather than as the empty
  # directories seen beforehand: `-delete` empties parents as it descends.
  before=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  /usr/bin/find "$backup" -mindepth 1 -type d -empty -delete 2>/dev/null
  after=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  pruned=$((before - after))
fi
printf 'STADO_BACKUP_PRUNED\t%s\n' "$pruned"
printf 'STADO_BACKUP_FREE\t%s\t%s\n' 'after' "$(free_kb)"
"#;

/// One pass over one host's replica, as the operator asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPlan {
    /// Namespace a bare replica path maps into on the primary side.
    pub namespace: String,
    /// Replica root, relative to the remote login user's `$HOME`.
    pub backup_root: String,
    /// Primary store root, relative to the same `$HOME`.
    pub primary_root: String,
    /// Exact namespace-qualified object paths to compare. Empty scans the
    /// replica as before.
    pub objects: Vec<String>,
    /// Namespaces whose backup-visible object paths and size metadata should be
    /// listed without reading object bodies.
    pub inventory_namespaces: Vec<String>,
    /// Also delete the twins this pass proves.
    pub reclaim: bool,
    /// Actually delete. Without it a reclaim names what it would drop and
    /// drops nothing.
    pub apply: bool,
}

impl AuditPlan {
    /// The hashing budget this pass needs: a reclaim must prove everything it
    /// deletes, a read-only pass may stop early and label the rest.
    fn hash_deadline_seconds(&self) -> u64 {
        if self.reclaim {
            RECLAIM_HASH_DEADLINE_SECONDS
        } else {
            HASH_DEADLINE_SECONDS
        }
    }
}

/// The remote program with this host's roots, namespace, hashing deadline and
/// reclaim mode in place.
pub fn remote_script(plan: &AuditPlan) -> String {
    REMOTE_SCRIPT_TEMPLATE
        .replace(NAMESPACE_MARK, &plan.namespace)
        .replace(BACKUP_ROOT_MARK, &plan.backup_root)
        .replace(PRIMARY_ROOT_MARK, &plan.primary_root)
        .replace(
            OBJECTS_HEX_MARK,
            &plan
                .objects
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join(","),
        )
        .replace(
            INVENTORY_NAMESPACES_HEX_MARK,
            &plan
                .inventory_namespaces
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join(","),
        )
        .replace("@HASH_DEADLINE@", &plan.hash_deadline_seconds().to_string())
        .replace("@RECLAIM@", if plan.reclaim { "yes" } else { "no" })
        // A pass that was not asked to reclaim cannot apply anything, whatever
        // else it was handed.
        .replace(
            "@APPLY@",
            if plan.reclaim && plan.apply {
                "yes"
            } else {
                "no"
            },
        )
}

/// One class's totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassTotals {
    pub objects: u64,
    pub bytes: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectIdentity {
    pub state: String,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectComparison {
    pub path: String,
    pub primary: ObjectIdentity,
    pub backup: ObjectIdentity,
    pub primary_metadata: ObjectIdentity,
    pub backup_metadata: ObjectIdentity,
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
    /// Exact primary/backup identities requested by the operator.
    pub objects: Vec<ObjectComparison>,
    /// Backup-visible paths and size metadata from explicitly selected
    /// namespaces. SHA-256 is intentionally absent because this inventory does
    /// not read object bodies.
    pub inventory_objects: Vec<ObjectComparison>,
    /// Immediate directory children under each physical root's `ecosystem/`.
    /// This is metadata-only and proves which API namespaces were considered
    /// without walking or reading their object bodies.
    pub namespaces: BTreeMap<String, Vec<String>>,
    /// True only when both fixed physical roots completed that directory read.
    pub namespace_inventory_complete: bool,
    /// Set when the host could not be classified at all.
    pub unavailable: Option<String>,
    /// True once the remote program printed its end marker, so a truncated
    /// channel is never read as "nothing to reclaim".
    pub complete: bool,
    /// What the pass deleted, and what it would have deleted without
    /// `--apply`. Both are the pass's OWN proof: an object counted here was
    /// hashed on both sides moments before the unlink.
    pub deleted: ClassTotals,
    pub would_delete: ClassTotals,
    /// Deletions the host refused, which leave the replica object in place.
    pub delete_failed: ClassTotals,
    /// Emptied replica directories removed after the deletions.
    pub pruned_directories: i64,
    /// Free 1024-byte blocks on the replica's filesystem, read by this pass on
    /// both sides of its own work.
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// True once the reclaim half printed its own end marker.
    pub reclaim_complete: bool,
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
    let mut namespace_roots_complete = BTreeSet::new();
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
            Some("STADO_BACKUP_OBJECT") => {
                let (
                    Some(path),
                    Some(primary_state),
                    Some(primary_bytes),
                    Some(primary_sha256),
                    Some(backup_state),
                    Some(backup_bytes),
                    Some(backup_sha256),
                    Some(primary_metadata_state),
                    Some(primary_metadata_bytes),
                    Some(primary_metadata_sha256),
                    Some(backup_metadata_state),
                    Some(backup_metadata_bytes),
                    Some(backup_metadata_sha256),
                ) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                )
                else {
                    continue;
                };
                let identity = |state: &str, bytes: &str, sha256: &str| ObjectIdentity {
                    state: state.to_string(),
                    bytes: bytes.parse().ok(),
                    sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
                };
                audit.objects.push(ObjectComparison {
                    path: path.to_string(),
                    primary: identity(primary_state, primary_bytes, primary_sha256),
                    backup: identity(backup_state, backup_bytes, backup_sha256),
                    primary_metadata: identity(
                        primary_metadata_state,
                        primary_metadata_bytes,
                        primary_metadata_sha256,
                    ),
                    backup_metadata: identity(
                        backup_metadata_state,
                        backup_metadata_bytes,
                        backup_metadata_sha256,
                    ),
                });
            }
            Some("STADO_BACKUP_INVENTORY_OBJECT") => {
                let (
                    Some(encoded_path),
                    Some(primary_state),
                    Some(primary_bytes),
                    Some(primary_sha256),
                    Some(backup_state),
                    Some(backup_bytes),
                    Some(backup_sha256),
                    Some(primary_metadata_state),
                    Some(primary_metadata_bytes),
                    Some(primary_metadata_sha256),
                    Some(backup_metadata_state),
                    Some(backup_metadata_bytes),
                    Some(backup_metadata_sha256),
                ) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                )
                else {
                    continue;
                };
                let Ok(path_bytes) = hex::decode(encoded_path) else {
                    continue;
                };
                let Ok(path) = String::from_utf8(path_bytes) else {
                    continue;
                };
                let identity = |state: &str, bytes: &str, sha256: &str| ObjectIdentity {
                    state: state.to_string(),
                    bytes: bytes.parse().ok(),
                    sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
                };
                audit.inventory_objects.push(ObjectComparison {
                    path,
                    primary: identity(primary_state, primary_bytes, primary_sha256),
                    backup: identity(backup_state, backup_bytes, backup_sha256),
                    primary_metadata: identity(
                        primary_metadata_state,
                        primary_metadata_bytes,
                        primary_metadata_sha256,
                    ),
                    backup_metadata: identity(
                        backup_metadata_state,
                        backup_metadata_bytes,
                        backup_metadata_sha256,
                    ),
                });
            }
            Some("STADO_BACKUP_NAMESPACE") => {
                let (Some(root), Some(encoded)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let Ok(bytes) = hex::decode(encoded) else {
                    continue;
                };
                let Ok(namespace) = String::from_utf8(bytes) else {
                    continue;
                };
                audit
                    .namespaces
                    .entry(root.to_string())
                    .or_default()
                    .push(namespace);
            }
            Some("STADO_BACKUP_NAMESPACES_END") => {
                if let Some(root) = fields.next() {
                    namespace_roots_complete.insert(root.to_string());
                }
            }
            Some("STADO_BACKUP_NAMESPACES_ERROR") => {
                let root = fields.next().unwrap_or("unknown");
                let detail = fields.next().unwrap_or("namespace inventory failed");
                audit.unavailable = Some(format!("{root}: {detail}"));
            }
            Some("STADO_BACKUP_AUDIT_UNAVAILABLE") => {
                audit.unavailable = fields.next().map(str::to_string);
            }
            Some("STADO_BACKUP_AUDIT_END") => audit.complete = true,
            Some("STADO_BACKUP_RECLAIM") => {
                let (Some(outcome), Some(size), Some(_path)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                let bytes = size.trim().parse::<u64>().unwrap_or_default();
                let totals = match outcome {
                    "deleted" => &mut audit.deleted,
                    "would_delete" => &mut audit.would_delete,
                    "delete_failed" => &mut audit.delete_failed,
                    _ => continue,
                };
                totals.objects += 1;
                totals.bytes += bytes;
            }
            Some("STADO_BACKUP_RECLAIM_END") => audit.reclaim_complete = true,
            Some("STADO_BACKUP_PRUNED") => {
                audit.pruned_directories = fields
                    .next()
                    .and_then(|count| count.trim().parse::<i64>().ok())
                    .unwrap_or_default();
            }
            Some("STADO_BACKUP_FREE") => {
                let (Some(phase), Some(blocks)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let blocks = blocks.trim().parse::<i64>().ok();
                match phase {
                    "before" => audit.free_kb_before = blocks,
                    "after" => audit.free_kb_after = blocks,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for namespaces in audit.namespaces.values_mut() {
        namespaces.sort();
        namespaces.dedup();
    }
    audit.namespace_inventory_complete = ["local_storage", "local_backup"]
        .iter()
        .all(|root| namespace_roots_complete.contains(*root));
    audit
}

/// Classify `host`'s replica against its primary store, and — when the plan
/// says so — delete the twins the same pass just proved.
///
/// The proof and the deletion are one pass on purpose. An audit written to a
/// file and a deletion run against it later is how a safety net becomes data
/// loss: the addresses move, the primary changes, and the recorded verdict
/// stops describing the disk. Nothing in this module can act on a verdict it
/// did not compute in the same run.
pub async fn audit_host(
    host: &str,
    plan: &AuditPlan,
    runner: &Runner,
) -> Result<(ComputeTarget, BackupAudit), DeployError> {
    let target = host_channel::canonical_target(host).await?;
    let script = remote_script(plan);
    let output = if plan.reclaim {
        host_channel::run_script_with_timeout(
            &target,
            &script,
            Duration::from_secs(RECLAIM_TIMEOUT_SECONDS),
            runner,
        )
        .await?
    } else {
        host_channel::run_script(&target, &script, runner).await?
    };
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not classify its replica",
        )));
    }
    let audit = parse_output(&output.stdout, &target.name);
    Ok((target, audit))
}

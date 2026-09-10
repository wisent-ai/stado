//! `stado space reclaim TARGET [--stage STAGE]... [--dry-run|--apply]` gets
//! disk space back in measured, auditable stages.
//!
//! The selectable vocabulary and its ordering come from
//! `stado-rs/data/fleet/space.json`, compiled into the binary. The remote program
//! contains each stage's guarded implementation, while one `stage_enabled`
//! predicate selects declaration rows without a command-side match arm. A new
//! target product is therefore a declaration change, not another CLI verb.
//!
//! `registry_cleanup` runs the target's own janitor and consequently reads its
//! cleaner policy from the canonical registry. The other declared stages cover
//! build scratch, queue workdirs, foreign home trees, delivered product trees,
//! rebuildable caches, Chromium clones, local APFS snapshots, and runner work
//! trees. Every candidate remains constrained to its stage's product-owned or
//! operating-system-owned root.
//!
//! Dry-run is the default. Apply mode uses the identical enumeration, gates the
//! actual removal behind the mode bit, and is audited on the target whose state
//! changed.
//!
//! Five rules, encoded here rather than left to whoever is at the keyboard:
//!
//! - **nothing outside the stage roots.** Every candidate is produced by
//!   traversing a fixed product or operating-system root; no path arrives from
//!   the registry, the operator, or the host's own output. The one exception is
//!   named and constrained: the clone container is the OS's own answer for this
//!   account (`$TMPDIR`, `getconf DARWIN_USER_TEMP_DIR` behind it), and the
//!   stage refuses it unless it is under `/var/folders`, which is the only
//!   place macOS puts one.
//! - **one enumeration, not two.** Candidates and the newest-tree guard come
//!   from the SAME glob, and the age gate is asked per candidate. A `find` for
//!   one and a glob for the other differ by exactly the dotted entries, and on
//!   this control plane's own host that difference named
//!   `.macos-capability-backup-20260803` — an operator's state backup sitting
//!   beside the deliveries, which no delivery created and no reclamation may
//!   take. A delivery and a build both produce plainly named directories, so
//!   the glob IS the set.
//! - **never a path a live process holds.** One `ps` snapshot is taken before
//!   any stage and every candidate is checked against it. Taken once, into a
//!   variable, because `ps | grep <path>` matches the grep's own argv and would
//!   report every candidate as held.
//! - **never the newest tree of a product, and never what `current` resolves
//!   to.** Both are kept even when they are the largest thing there, and
//!   nothing younger than [`MIN_AGE_DAYS`] is touched at all, which is what
//!   makes the stage safe against a delivery that is mid-flight: its tree is
//!   the newest one and the youngest one.
//! - **`--dry-run` deletes nothing.** It is the default, and the same script
//!   runs in both modes with the removal itself behind the mode flag, so a
//!   preview walks exactly the paths an apply would remove rather than a
//!   second implementation's guess at them.
//!
//! The remote program is a raw Rust string: the `\t` and `\n` in its `printf`
//! formats are the literal backslash sequences the remote shell expands, and
//! spelling a program this size through escaped quotes is how a marker gets
//! silently mistyped.
//!
//! The components: `declaration` parses the compiled stage vocabulary,
//! `program` holds that raw program and its substitution, `outcome` folds the
//! host's marker lines into the report, and `session` runs one reclamation and
//! records it.

mod declaration;
mod outcome;
mod program;
mod session;

pub use declaration::{declared_stages, select_stages, StageDeclaration, DECLARATION_PATH};
pub use outcome::{to_report, Reclamation, Stage};
pub use session::{reclaim_host, record_audit};

/// The release build scratch tree, relative to the target account's home.
///
/// Its own root under `.stado`, and the one the fleet's checked-in build
/// helper already uses (`scripts/build-stado-linux-host.sh`): a stage that
/// reclaimed a directory that helper does not write would be reclaiming
/// something else.
pub const BUILD_WORK_ROOT: &str = ".stado/build-work";

/// Nothing younger than this is a candidate, in any stage.
///
/// A build in flight and a delivery in flight both keep their own directory
/// fresh, so age is the guard that does not depend on a process being visible
/// to `ps` at the instant the sweep runs.
pub const MIN_AGE_DAYS: &str = "1";
/// Chromium creates more than 100 full-bundle clones in a day on an active
/// Weles host. Process ownership and newest-clone guards make one hour enough
/// to survive launch races without allowing the clone root to fill the disk.
pub const CLONE_MIN_AGE_MINUTES: &str = "60";

/// `mode` for a run that measured and removed nothing.
pub const DRY_RUN_MODE: &str = "dry_run";
/// `mode` for a run that removed what its stages named.
pub const APPLY_MODE: &str = "apply";

/// The only prefix a macOS temporary container has, and the guard on the one
/// root this module does not spell itself.
///
/// The container comes from the OS (`$TMPDIR`, `getconf DARWIN_USER_TEMP_DIR`
/// behind it) because nothing else knows where it is — its name carries a
/// per-account hash. A value from outside that is not under this prefix is not
/// a container, and the stage walks nothing rather than trusting it.
pub const CONTAINER_PREFIX: &str = "/var/folders/";

/// Suffix a stage name carries when the host could not run it at all.
///
/// A stage that did not run is reported UNDER ITS OWN NAME plus this suffix,
/// with null measurements, rather than omitted or folded into a zero-item
/// success. "Nobody looked" rendered as "nothing was there" is the fold this
/// fleet has already paid for once.
pub const UNAVAILABLE_SUFFIX: &str = "_unavailable";

/// Where the record of an applied reclamation lands on the host whose disk
/// changed, relative to that account's home.
///
/// On the machine, next to the state it changed, and not in a central ledger:
/// the disk that moved is the host's, the operator who moved it may never touch
/// this control plane again, and a record kept anywhere else is a record that
/// can be missing exactly when someone asks what happened to that box.
pub const AUDIT_LOG: &str = ".stado/audit/host-reclaim.jsonl";

/// Current agents retain queue workdirs under `$HOME/.stado/work/jobs`; older
/// agents used `/tmp` or the per-user temporary container. All three roots
/// carry the same queue-authority and process-liveness proof policy.
pub const DEFAULT_WORK_ROOTS: &str = "\"$HOME/.stado/work/jobs\" /tmp \"${TMPDIR:-}\"";
pub const LOCAL_EVIDENCE_ROOT: &str = ".stado/work/host-reclaim-local-evidence";
pub const LOCAL_TERMINALITY_GRACE_SECONDS: i64 = crate::config::HEARTBEAT_STALE_MINUTES * 60;

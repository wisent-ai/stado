//! Which mechanism reaches bytes no reclamation stage covers.
//!
//! Two different mechanisms clean a Stado host, and the coverage report knew
//! about one of them. `stado space reclaim` runs the stages compiled from
//! `data/space.json`; the janitor runs the cleaners a host declares under
//! `targets[].disk_cleanup.cleaners`. A path outside every stage root was
//! therefore printed as a path "where no declared stage looks", which reads as
//! "nothing in this product can take these bytes".
//!
//! On 2026-09-09 that sentence was false on `charless-mac-mini`. The report
//! named `~/.stado/local-storage` at 52.4 GiB and `~/.stado/local-backup` at
//! 10.4 GiB as unswept, and that host declares all seven cleaners this product
//! implements — including `release_store`, whose root is inside the first path,
//! and `backup_twins`, whose root is the second. The bytes were not stranded;
//! the janitor's last pass had stopped at `cap_reached`, its own per-pass
//! budget. Those are opposite repairs: one needs a declaration, the other needs
//! the pass to keep going.
//!
//! So every unswept path is classified against the catalogue: a cleaner this
//! host declares, a cleaner this product implements and the host has not
//! declared, or nothing at all.

use super::paths::{self, Occupant};
use crate::providers::local::disk_cleanup::catalogue;

/// One cleaner as a host declares it: the name, and the root that declaration
/// overrides the catalogue default with.
///
/// The override matters and was missed once. `charless-mac-mini` declares
/// `weles_recordings` rooted at `/Users/charles/.stado/var/weles/recordings`,
/// not the catalogue's `~/.stado/weles/recordings`, so a reach test that read
/// only the default printed `~/.stado/var` as a directory nothing looks at
/// while a declared cleaner was sweeping inside it.
#[derive(Debug, Clone)]
pub struct DeclaredCleaner {
    pub name: String,
    pub root: Option<String>,
}

/// What can reach one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// A cleaner this host declares owns it; a pass can take it.
    Declared(&'static str),
    /// This product implements a cleaner for it and the host declares none.
    Unarmed(&'static str),
    /// No mechanism in this product looks here.
    Nothing,
}

impl Reach {
    pub fn cleaner(self) -> Option<&'static str> {
        match self {
            Self::Declared(name) | Self::Unarmed(name) => Some(name),
            Self::Nothing => None,
        }
    }

    pub fn is_declared(self) -> bool {
        matches!(self, Self::Declared(_))
    }
}

/// Where a cleaner sweeps on this host: the declared root when the host
/// overrides one, the catalogue default otherwise.
fn root_of(
    entry: &catalogue::CleanerDeclaration,
    home: &str,
    declared: &[DeclaredCleaner],
) -> Option<String> {
    if let Some(root) = declared
        .iter()
        .find(|row| row.name == entry.name)
        .and_then(|row| row.root.clone())
    {
        return Some(paths::absolute(&root, home));
    }
    if entry.default_root.is_empty() {
        return None;
    }
    Some(paths::absolute(&format!("~/{}", entry.default_root), home))
}

/// Whether a cleaner rooted at `root` touches `path` at all: the same
/// directory, the cleaner inside the path, or the path inside the cleaner.
fn touches(root: &str, path: &str) -> bool {
    root == path || paths::within(root, path) || paths::within(path, root)
}

/// Classify one path. A declared cleaner wins over a merely implemented one,
/// because the question a reader has is whether a pass can take these bytes
/// today.
pub(super) fn reach(path: &str, home: &str, declared: &[DeclaredCleaner]) -> Reach {
    let mut implemented = Reach::Nothing;
    for entry in catalogue::CLEANERS {
        let Some(root) = root_of(entry, home, declared) else {
            continue;
        };
        if !touches(&root, path) {
            continue;
        }
        if declared.iter().any(|row| row.name == entry.name) {
            return Reach::Declared(entry.name);
        }
        if implemented == Reach::Nothing {
            implemented = Reach::Unarmed(entry.name);
        }
    }
    implemented
}

/// One cleaner this product implements, undeclared, whose root holds unswept
/// bytes.
pub(super) struct Unarmed {
    pub cleaner: &'static str,
    pub root: String,
    pub since: &'static str,
    /// Whether the binary installed on the target can take the declaration.
    pub supported: bool,
    /// Bytes the inventory measured at the root itself.
    pub bytes: Option<i64>,
    /// When the walk stopped above the root, the row that contains it: bytes
    /// standing at or above the cleaner's root, never claimed as its own.
    pub within_path: Option<String>,
    pub within_bytes: Option<i64>,
}

impl Unarmed {
    /// The best figure known for this root, for ordering and for the sentence.
    pub fn known_bytes(&self) -> i64 {
        self.bytes.or(self.within_bytes).unwrap_or_default()
    }

    pub fn detail(&self) -> String {
        let sweeps = catalogue::cleaner(self.cleaner)
            .map(|entry| entry.sweeps)
            .unwrap_or_default();
        if !self.supported {
            return format!(
                "this product implements {} for {}, and the binary installed here predates it: deliver at least {} first",
                self.cleaner, self.root, self.since
            );
        }
        match self.within_path.as_ref() {
            Some(within) => format!(
                "{} sweeps {sweeps} under {}, inside the unswept {within}, and this host does not declare it",
                self.cleaner, self.root
            ),
            None => format!(
                "{} sweeps {sweeps} under {} and this host does not declare it",
                self.cleaner, self.root
            ),
        }
    }
}

/// Every implemented cleaner that could reach an unswept row, largest first.
pub(super) fn unarmed(
    occupants: &[Occupant],
    uncovered: &[Occupant],
    home: &str,
    declared: &[DeclaredCleaner],
    installed: &str,
) -> Vec<Unarmed> {
    let mut rows: Vec<Unarmed> = catalogue::CLEANERS
        .iter()
        .filter(|entry| !declared.iter().any(|row| row.name == entry.name))
        .filter_map(|entry| {
            let root = root_of(entry, home, declared)?;
            let bytes = paths::measured(&root, occupants);
            let container = uncovered.iter().find(|row| touches(&root, &row.path));
            if bytes.is_none() && container.is_none() {
                return None;
            }
            let separate = container.filter(|row| Some(row.bytes) != bytes);
            Some(Unarmed {
                cleaner: entry.name,
                root,
                since: entry.since,
                supported: catalogue::version_at_least(installed, entry.since),
                bytes,
                within_path: separate.map(|row| row.path.clone()),
                within_bytes: separate.map(|row| row.bytes),
            })
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.known_bytes()));
    rows
}

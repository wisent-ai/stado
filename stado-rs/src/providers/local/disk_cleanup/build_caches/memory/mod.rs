//! What earlier passes learned of the home, keyed by the identity and
//! change time of each directory: the child directories of every directory
//! the walk examined, and the measurement of every tagged cache it found.
//!
//! The walk examined every directory under the home on every pass under
//! disk pressure, once a minute, on a host whose tree had not changed:
//! listing each directory and reading every entry's kind. A directory's
//! inode, device and modification time name its listing: a directory adds
//! or loses an entry only by changing its own time, so a directory whose
//! three are what they were has the child directories it had, and the walk
//! takes them from memory instead of listing it. Each child is still opened,
//! checked and read for its tag as before; only the listing and the kind
//! of every non-directory entry are not read again. A new tree anywhere
//! changes the time of the directory it appears in, which is listed again.
//!
//! Measuring a cache means reading every file beneath it, so the bytes of a
//! cache whose root is what it was are reported from memory too; a file
//! growing inside an existing subdirectory does not change the root's time,
//! so the remembered bytes are those of the last measurement until the root
//! changes or the cache is removed. Reclamation never reads the memory: an
//! enforcing pass removes the tree and measures the volume before and after.
//!
//! The memory lives beside the janitor's state (`build-caches.json` in its
//! state directory). A completed pass rewrites it with what it found, so a
//! directory or cache that disappeared is forgotten with the pass that no
//! longer found it; a pass that stopped early leaves the earlier memory.
//! A memory that learned nothing new is not rewritten.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nix::libc;
use serde::{Deserialize, Serialize};

/// The file's name in the janitor's state directory.
pub const MEMO_NAME: &str = "build-caches.json";

/// One remembered measurement.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Remembered {
    pub device: u64,
    pub inode: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub bytes: i64,
    pub measured_at: String,
}

/// The identity and change time that name one directory's listing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
}

impl Identity {
    fn of(info: &libc::stat) -> Self {
        Self {
            // dev_t is u64 on Linux and i32 on macOS; the cast is the platform
            // difference, not a redundancy.
            device: info.st_dev as u64,
            inode: info.st_ino,
            modified_seconds: info.st_mtime,
            modified_nanoseconds: info.st_mtime_nsec,
        }
    }
}

impl Remembered {
    fn names(&self, info: &libc::stat) -> bool {
        let identity = Identity::of(info);
        self.device == identity.device
            && self.inode == identity.inode
            && self.modified_seconds == identity.modified_seconds
            && self.modified_nanoseconds == identity.modified_nanoseconds
    }
}

/// One examined directory: what named its listing and the child directories
/// the listing held, as names relative to it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Listing {
    pub identity: Identity,
    pub directories: Vec<PathBuf>,
}

/// What the janitor remembers of the home it walks.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Memo {
    #[serde(default)]
    pub caches: BTreeMap<PathBuf, Remembered>,
    /// Every examined directory, by its path relative to the home.
    #[serde(default)]
    pub listings: BTreeMap<PathBuf, Listing>,
    /// The caches this pass found, rewritten over `caches` when the pass
    /// completed; a cache a stopped pass did not reach is not forgotten.
    #[serde(skip)]
    found: BTreeMap<PathBuf, Remembered>,
    /// The directories this pass examined, rewritten over `listings` when
    /// the pass completed.
    #[serde(skip)]
    examined: BTreeMap<PathBuf, Listing>,
    /// The findings this pass answered from memory instead of reading.
    #[serde(skip)]
    answered: BTreeSet<PathBuf>,
    /// Whether this pass listed or measured anything the memory did not
    /// hold, so an unchanged memory is not rewritten.
    #[serde(skip)]
    learned: bool,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Memo {
    /// The memory beside the janitor's state, or an empty one for a state
    /// directory that holds none yet. An unreadable file is an error: a
    /// measurement nobody can read is not the same as none.
    pub fn open(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join(MEMO_NAME);
        let mut memo = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes).map_err(|error| {
                format!(
                    "the build cache memory {} cannot be read: {error}",
                    path.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                return Err(format!(
                    "the build cache memory {} cannot be opened: {error}",
                    path.display()
                ))
            }
        };
        memo.path = Some(path);
        Ok(memo)
    }

    /// A memory kept nowhere: a preview, or a walk without a state directory.
    pub fn transient() -> Self {
        Self::default()
    }

    /// The remembered bytes of the cache at `root`, when its root directory
    /// is the one measured; the entry is carried into this pass's findings.
    pub fn recall(&mut self, root: &Path, info: &libc::stat) -> Option<i64> {
        let remembered = self.caches.get(root).filter(|entry| entry.names(info))?;
        let bytes = remembered.bytes;
        self.found.insert(root.to_path_buf(), remembered.clone());
        self.answered.insert(root.to_path_buf());
        Some(bytes)
    }

    /// Record a measurement just taken.
    pub fn measured(&mut self, root: &Path, info: &libc::stat, bytes: i64) {
        let identity = Identity::of(info);
        self.found.insert(
            root.to_path_buf(),
            Remembered {
                device: identity.device,
                inode: identity.inode,
                modified_seconds: identity.modified_seconds,
                modified_nanoseconds: identity.modified_nanoseconds,
                bytes,
                measured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            },
        );
        self.learned = true;
    }

    /// The child directories of the directory at `relative` (to the home),
    /// when the directory is the one listed before; the listing is carried
    /// into this pass's examined set.
    pub fn remembered_directories(
        &mut self,
        relative: &Path,
        info: &libc::stat,
    ) -> Option<Vec<PathBuf>> {
        let identity = Identity::of(info);
        let listing = self
            .listings
            .get(relative)
            .filter(|listing| listing.identity == identity)?;
        let directories = listing.directories.clone();
        self.examined
            .insert(relative.to_path_buf(), listing.clone());
        Some(directories)
    }

    /// Record a listing just read.
    pub fn listed(&mut self, relative: &Path, info: &libc::stat, directories: Vec<PathBuf>) {
        self.examined.insert(
            relative.to_path_buf(),
            Listing {
                identity: Identity::of(info),
                directories,
            },
        );
        self.learned = true;
    }

    /// How many directories this pass took from memory instead of listing.
    pub fn listings_remembered(&self) -> usize {
        self.examined
            .keys()
            .filter(|relative| {
                self.listings
                    .get(*relative)
                    .is_some_and(|listing| Some(listing) == self.examined.get(*relative))
            })
            .count()
    }

    /// A cache this pass removed is forgotten.
    pub fn removed(&mut self, root: &Path) {
        self.found.remove(root);
        self.caches.remove(root);
        self.learned = true;
    }

    /// How many caches this pass found so far, remembered or measured.
    pub fn found(&self) -> usize {
        self.found.len()
    }

    /// How many of this pass's findings were answered from memory.
    pub fn remembered(&self) -> usize {
        self.answered.len()
    }

    /// Keep this pass's findings as the memory when the walk reached its
    /// end; a walk that stopped early keeps the earlier memory, since the
    /// caches it did not reach are still there. A memory that learned
    /// nothing and forgot nothing is left as written.
    pub fn keep(mut self, completed: bool) -> Result<(), String> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        if !completed {
            return Ok(());
        }
        let forgot =
            self.found.len() != self.caches.len() || self.examined.len() != self.listings.len();
        if !self.learned && !forgot {
            return Ok(());
        }
        self.caches = std::mem::take(&mut self.found);
        self.listings = std::mem::take(&mut self.examined);
        let document = serde_json::to_value(&self)
            .map_err(|error| format!("the build cache memory cannot be serialized: {error}"))?;
        stado_product::common::atomic_json(&path, &document).map_err(|error| {
            format!(
                "the build cache memory {} cannot be written: {error:#}",
                path.display()
            )
        })
    }
}

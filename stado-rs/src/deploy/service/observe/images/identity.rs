// ---------------------------------------------------------------------------
// Which image a unit's live process is executing, on this machine
// ---------------------------------------------------------------------------

/// The three directories this fleet installs launchd units into, in the
/// order [`crate::deploy::service::LOADED_UNITS_SCRIPT`] walks them.
///
/// Same list and same order deliberately: a unit one enumeration can see and
/// the other cannot is how a label ends up in nobody's set, which is the
/// defect `service list --undeclared` was built for.
pub(crate) const LAUNCHD_UNIT_DIRECTORIES: [&str; 3] = [
    "/Library/LaunchDaemons",
    "$HOME/Library/LaunchAgents",
    "/Library/LaunchAgents",
];

/// The two directories this fleet writes systemd units into: system units
/// through `sudo tee` (bootstrap, stream, storage reconcile) and user units
/// through the service deploy and ensure scripts.
pub(crate) const SYSTEMD_UNIT_DIRECTORIES: [&str; 2] =
    ["/etc/systemd/system", "$HOME/.config/systemd/user"];

/// One executable file, as the kernel identifies it rather than as a path
/// spells it.
///
/// A path is not an identity, and that gap is the whole condition this type
/// exists to express: two different files answering to one name. Every field
/// here is here because a path comparison cannot see it — which is why
/// [`RunningProgram::matches_process`], which compares paths and then
/// timestamps, reports a unit whose binary was swapped underneath it as
/// matching, and why `recycle_launchd` decides what to restart by string
/// equality on `argv[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageIdentity {
    /// Where the identity was read: the declared path for an installed file,
    /// and whatever the kernel still calls the mapping for a running one.
    pub path: String,
    /// `st_dev`. Inode numbers repeat across volumes, so the pair is the
    /// identity and the inode on its own is not.
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    /// Directory entries pointing at this inode. Zero means the file has been
    /// unlinked and the running process holds the last reference to the bytes
    /// it is executing — a different operator problem from a process running
    /// some other file that still exists, so the two are never merged.
    pub links: u64,
}

impl ImageIdentity {
    /// The same file, by the only test that answers it.
    pub fn is_same_file(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }

    /// The identity in one clause, so a report can print both sides and be
    /// believed without anybody going back to `lsof`.
    pub fn describe(&self) -> String {
        format!(
            "inode {} on device {:#x}, {} bytes, {} link(s)",
            self.inode, self.device, self.bytes, self.links
        )
    }
}

/// What a managed unit's live process turned out to be executing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    /// The executing file has no name left. The directory entry now points at
    /// other bytes and the running process holds the only remaining reference
    /// to the ones it is executing.
    ///
    /// This is the case of a long-lived janitor `--watch` process executing
    /// an inode with zero links while `~/.stado/bin/stado` has been replaced
    /// more than once underneath it. It is a variant of its own because it
    /// is the one where no copy of
    /// the running build survives anywhere to be diffed.
    Unlinked {
        running: ImageIdentity,
        installed: ImageIdentity,
    },
    /// The executing file still exists and is not the one the unit declares —
    /// an artefact tree a `current` link no longer points at, or a second copy
    /// of the same program elsewhere on the disk.
    Replaced {
        running: ImageIdentity,
        installed: ImageIdentity,
    },
    /// The identity could not be established.
    ///
    /// A finding and never a silence. The defect this check exists to remove
    /// is an unread state rendered as a passing one, and `registry doctor`
    /// already applies the same rule to unit files it cannot open:
    /// [`EnvironmentGap::UnrecordedDeclaration`] carries a non-`Read`
    /// [`UnitReading`] and says the file was not read rather than printing
    /// an empty environment.
    Unread {
        /// What could not be read, named the way an operator would name it.
        subject: String,
        /// The reader's own words for why, never a paraphrase.
        reason: String,
    },
}

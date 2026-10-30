//! Scratch that a job, an agent or a Stado verb created on this host and
//! declared it may lose: the register that makes such a tree the fleet's to
//! clean.
//!
//! Every other cleaner knows its area by a fixed path or by a tool's own
//! `CACHEDIR.TAG`. What a coding agent exports to `<repository>/.build/`
//! carries neither, and neither does what a verb brings back beside a
//! checkout, so nothing ever took them. A host its owner had cleaned below
//! the disk-full rule filled up again with trees nobody had registered and
//! nothing removed.
//!
//! A creator registers each tree it makes with `stado space scratch
//! register`, naming itself, why the tree exists, the session that owns it
//! and that session's process. Above the disk-full rule the janitor removes
//! every registered tree whose owner process has ended, least recently
//! changed first, records each removal in its removal record
//! ([`super::candidates::ledger`]), and forgets the rows whose tree is gone.
//! A tree that was never registered is never taken here: what the person
//! keeps stays theirs.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::candidates::ledger;
use super::janitor::state::report::{met::Met, CleanupReport};
use super::weles::{dir_size, remove_tree};
use super::{JanitorError, STATE_DIR_PARTS};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "registered_scratch";

/// Home-relative file holding the register.
pub const REGISTER: &str = ".stado/space/scratch.json";

/// The register document's version this build reads and writes.
const SCHEMA_VERSION: u32 = 1;

/// One registered tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchEntry {
    /// Absolute path of the tree as registered. It is never resolved through
    /// a link afterwards: a link at that path is refused, not followed.
    pub path: PathBuf,
    /// Who made it: a product, an agent or a Stado verb.
    pub creator: String,
    /// Why it exists, in the creator's words.
    pub purpose: String,
    /// When it was registered, RFC 3339.
    pub registered_at: String,
    /// The session that owns it: an agent session, a job, a verb's run.
    pub owner_session: String,
    /// The owner session's process. While it runs the tree is kept.
    pub owner_pid: i32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterDocument {
    schema_version: u32,
    entries: Vec<ScratchEntry>,
}

/// The register file under `home`.
pub fn register_path(home: &Path) -> PathBuf {
    home.join(REGISTER)
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Read the register. An absent file is an empty register; a file that does
/// not parse or names another schema is refused with its path and the fix.
pub fn read(home: &Path) -> io::Result<Vec<ScratchEntry>> {
    let path = register_path(home);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "the scratch register {} cannot be read: {error}; it must be this account's \
                     regular file",
                    path.display()
                ),
            ))
        }
    };
    let document: RegisterDocument = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a scratch register ({error}); only `stado space scratch` writes it, \
                 so it was edited or damaged, and no registered tree is taken until it is \
                 read: move it aside and register the trees again",
                path.display()
            ),
        )
    })?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} declares schema_version {}, and this build reads {SCHEMA_VERSION}; run the \
                 Stado release that wrote it, or move it aside and register the trees again",
                path.display(),
                document.schema_version
            ),
        ));
    }
    Ok(document.entries)
}

/// Change the register under its own lock: one writer at a time, and the new
/// document replaces the old one whole, so a reader never sees half of it.
fn update<T>(
    home: &Path,
    change: impl FnOnce(&mut Vec<ScratchEntry>) -> io::Result<T>,
) -> io::Result<T> {
    let path = register_path(home);
    let directory = path.parent().ok_or_else(|| {
        invalid(format!(
            "the scratch register {} has no directory",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(directory)?;
    let lock = File::create(directory.join("scratch.lock"))?;
    lock.lock()?;
    let mut entries = read(home)?;
    let answer = change(&mut entries)?;
    let document = RegisterDocument {
        schema_version: SCHEMA_VERSION,
        entries,
    };
    crate::primitives::document::persist(&path, &serde_json::to_vec_pretty(&document)?)?;
    Ok(answer)
}

/// Register `path` as scratch `creator` made for `owner_session`, kept while
/// `owner_pid` runs. The path must be an absolute directory below the home,
/// named by its own real path (no `.` or `..` and no link on the way), and
/// not inside Stado's own tree, the janitor's state or `~/.ssh`;
/// registering it again replaces the earlier row.
pub fn register(
    home: &Path,
    path: &Path,
    creator: &str,
    purpose: &str,
    owner_session: &str,
    owner_pid: i32,
) -> io::Result<ScratchEntry> {
    if !path.is_absolute() {
        return Err(invalid(format!(
            "{} is not an absolute path; name the tree from the root",
            path.display()
        )));
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return Err(invalid(format!(
            "{} names `.` or `..`, so the tree it reaches is not the one it spells; name the \
             tree by its own path",
            path.display()
        )));
    }
    if creator.trim().is_empty() || purpose.trim().is_empty() || owner_session.trim().is_empty() {
        return Err(invalid(
            "a registration names its creator, its purpose and its owner session: pass \
             --creator, --purpose and --owner-session, each non-empty"
                .to_string(),
        ));
    }
    if owner_pid <= 0 {
        return Err(invalid(format!(
            "--owner-pid {owner_pid} names no process; give the process of the session that \
             owns the tree (the agent's or job's own process, not a shell that ends with the \
             command)"
        )));
    }
    let info = std::fs::symlink_metadata(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "{} cannot be registered: {error}; create the tree first",
                path.display()
            ),
        )
    })?;
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err(invalid(format!(
            "{} is not a directory (a link is refused, not followed); register the directory \
             itself",
            path.display()
        )));
    }
    let real = |named: &Path, what: &str| {
        std::fs::canonicalize(named).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "{what} {} cannot be resolved to its real path: {error}",
                    named.display()
                ),
            )
        })
    };
    let real_path = real(path, "the tree")?;
    if real_path != path {
        return Err(invalid(format!(
            "{} reaches {} through a link on the way, so the janitor would remove a tree the \
             path does not spell; register the directory by its real path",
            path.display(),
            real_path.display()
        )));
    }
    let real_home = real(home, "the home")?;
    if !real_path.starts_with(&real_home) || real_path == real_home {
        return Err(invalid(format!(
            "{} is not below the home {}; only a tree inside this account's home can be its \
             scratch",
            path.display(),
            real_home.display()
        )));
    }
    let protected = [
        real_home.join(".stado"),
        real_home.join(STATE_DIR_PARTS[0]).join(STATE_DIR_PARTS[1]),
        real_home.join(".ssh"),
    ];
    if let Some(root) = protected.iter().find(|root| real_path.starts_with(root)) {
        return Err(invalid(format!(
            "{} is inside {}, which Stado's own cleaners manage or the owner keeps; register a \
             tree outside it",
            path.display(),
            root.display()
        )));
    }
    let entry = ScratchEntry {
        path: real_path,
        creator: creator.trim().to_owned(),
        purpose: purpose.trim().to_owned(),
        registered_at: chrono::Utc::now().to_rfc3339(),
        owner_session: owner_session.trim().to_owned(),
        owner_pid,
    };
    update(home, |entries| {
        entries.retain(|existing| existing.path != entry.path);
        entries.push(entry.clone());
        Ok(entry.clone())
    })
}

/// Forget `path` without touching it; whether a row was there.
pub fn forget(home: &Path, path: &Path) -> io::Result<bool> {
    update(home, |entries| {
        let before = entries.len();
        entries.retain(|existing| existing.path != path);
        Ok(entries.len() != before)
    })
}

/// Whether `pid` names a process that still runs. A recycled process id
/// reads as running, which keeps the tree: the safe direction.
fn alive(pid: i32) -> bool {
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        // The process exists but belongs to someone else.
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// One registered tree as it stands on disk now.
#[derive(Debug, Clone, Serialize)]
pub struct ScratchState {
    #[serde(flatten)]
    pub entry: ScratchEntry,
    /// Bytes under the tree, links not followed; `None` when it is gone.
    pub bytes: Option<i64>,
    /// When the tree itself last changed, RFC 3339; `None` when it is gone.
    pub modified_at: Option<String>,
    /// Whether the owner session's process still runs.
    pub owner_running: bool,
}

/// Every registered tree with its size, age and owner, least recently
/// changed first: the order the cleaner takes them in.
pub fn states(home: &Path) -> io::Result<Vec<ScratchState>> {
    let mut states: Vec<(Option<std::time::SystemTime>, ScratchState)> = read(home)?
        .into_iter()
        .map(|entry| {
            let info = std::fs::symlink_metadata(&entry.path)
                .ok()
                .filter(|info| info.is_dir() && !info.file_type().is_symlink());
            let modified = info.as_ref().and_then(|info| info.modified().ok());
            let state = ScratchState {
                bytes: info.as_ref().map(|_| dir_size(&entry.path)),
                modified_at: modified
                    .map(|stamp| chrono::DateTime::<chrono::Utc>::from(stamp).to_rfc3339()),
                owner_running: alive(entry.owner_pid),
                entry,
            };
            (modified, state)
        })
        .collect();
    states.sort_by_key(|(modified, _)| *modified);
    Ok(states.into_iter().map(|(_, state)| state).collect())
}

/// Remove every registered tree whose owner process has ended, oldest first,
/// and forget the rows whose tree is gone. `enforcing` false only measures.
/// Each removal is a line of the janitor's removal record in `state_dir`.
pub(in crate::providers::local::space_reclaim) fn scan_registered_scratch(
    home: &Path,
    state_dir: &Path,
    enforcing: bool,
    writer: &str,
    report: &mut CleanupReport,
) {
    let mut met = Met::default();
    let states = match states(home) {
        Ok(states) => states,
        Err(error) => {
            report.add_error(CLEANER, &JanitorError::os(&error.to_string()));
            return;
        }
    };
    met.listed(states.len());
    let mut gone: Vec<PathBuf> = Vec::new();
    for state in states {
        let Some(bytes) = state.bytes else {
            gone.push(state.entry.path.clone());
            met.skipped("already_gone");
            continue;
        };
        let size = u64::try_from(bytes).unwrap_or_default();
        if state.owner_running {
            met.kept("owner_session_running", size);
            continue;
        }
        met.eligible(size);
        if !enforcing {
            continue;
        }
        // The tree must still be the directory registered: a link put on
        // its way since would send the removal elsewhere.
        if std::fs::canonicalize(&state.entry.path).ok().as_deref()
            != Some(state.entry.path.as_path())
        {
            met.kept("path_now_leads_elsewhere", size);
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "{} no longer resolves to itself (a link was put on its way), so it is kept; \
                     `stado space scratch forget` it and register the real tree again",
                    state.entry.path.display()
                )),
            );
            continue;
        }
        match remove_tree(&state.entry.path) {
            Ok(()) => {
                met.removed(Some(size));
                gone.push(state.entry.path.clone());
                if let Err(error) =
                    ledger::record_tree(state_dir, CLEANER, writer, &state.entry.path, bytes, bytes)
                {
                    report.add_error("removal_record", &error);
                }
            }
            Err(error) => {
                met.skipped("deletion_refused");
                report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!(
                        "cannot remove {} ({} made it: {}): {error}",
                        state.entry.path.display(),
                        state.entry.creator,
                        state.entry.purpose
                    )),
                );
            }
        }
    }
    if enforcing && !gone.is_empty() {
        if let Err(error) = update(home, |entries| {
            entries.retain(|entry| !gone.contains(&entry.path));
            Ok(())
        }) {
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "removed trees stay listed in the scratch register: {error}"
                )),
            );
        }
    }
    match met.report() {
        Ok(record) => report.registered_scratch = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}

//! Opening a directory that macOS may gate behind a consent dialog, without
//! letting that dialog hold the janitor.
//!
//! `~/Desktop`, `~/Documents` and `~/Downloads` are consent-gated: the first
//! `open` under one of them by a process without the grant makes macOS ask
//! the person at the keyboard, and the syscall blocks until they answer. On
//! 2026-09-10 the agent on lukasz-macbook asked that question from its
//! janitor thread at 19:19Z and was still inside `openat` two hours later:
//! the pass held its exclusive lock the whole time, every admission on the
//! host reported `cleanup_in_progress`, and a required release delivery sat
//! queued while `host gates` said the host was claiming. A cleaner walking
//! the fleet's own checkouts under `~/Documents` is the declared policy, and
//! the grant is one decision for a stably signed binary; what must not happen
//! is the wait for that decision costing the host.
//!
//! The first open under each gated folder is therefore made on a helper
//! thread with a deadline. Answered in time, the folder is confirmed for this
//! process and every later open is direct. Not answered, the folder is
//! `pending`: the cleaner reports `consent_pending` and stops, the helper
//! thread stays with the dialog and records the answer when it comes, and no
//! later pass asks again while it is pending — one blocked thread per
//! process, never one per pass.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use super::safefs;

/// How long the first open under a gated folder may wait for the answer
/// before the pass gives the folder up for this run.
const CONSENT_WAIT: Duration = Duration::from_secs(5);

/// What one open under a gated folder came back with.
pub enum Gated {
    Opened(OwnedFd),
    /// The question is with the person at the keyboard; nothing under this
    /// folder is opened until it is answered.
    Pending,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Consent {
    Confirmed,
    Pending,
}

static CONSENT: LazyLock<Mutex<BTreeMap<PathBuf, Consent>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// The folders under `home` macOS gates behind a consent dialog and this
/// cleaner still walks, because build trees live in them.
#[cfg(target_os = "macos")]
pub fn gated_folders(home: &Path) -> Vec<PathBuf> {
    ["Desktop", "Documents", "Downloads"]
        .iter()
        .map(|part| home.join(part))
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn gated_folders(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

fn gated_folder<'a>(gated: &'a [PathBuf], absolute: &Path) -> Option<&'a PathBuf> {
    gated.iter().find(|folder| absolute.starts_with(folder))
}

fn state(folder: &Path) -> Option<Consent> {
    CONSENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(folder)
        .copied()
}

fn set_state(folder: &Path, consent: Option<Consent>) {
    let mut states = CONSENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match consent {
        Some(consent) => {
            states.insert(folder.to_path_buf(), consent);
        }
        None => {
            states.remove(folder);
        }
    }
}

/// `safefs::open_dir_at`, bounded when `absolute` is the first open under a
/// gated folder this process has not been answered for.
pub fn open_dir_at(
    gated: &[PathBuf],
    parent: RawFd,
    name: &OsStr,
    absolute: &Path,
) -> io::Result<Gated> {
    let Some(folder) = gated_folder(gated, absolute) else {
        return safefs::open_dir_at(parent, name).map(Gated::Opened);
    };
    match state(folder) {
        Some(Consent::Confirmed) => safefs::open_dir_at(parent, name).map(Gated::Opened),
        Some(Consent::Pending) => Ok(Gated::Pending),
        None => {
            let parent = safefs::dup_fd(parent)?;
            let name = name.to_os_string();
            bounded(folder, move || {
                let opened = safefs::open_dir_at(parent.as_raw_fd(), &name);
                drop(parent);
                opened
            })
        }
    }
}

/// `safefs::open_dir_path`, bounded the same way, for a cleaner's root.
pub fn open_dir_path(gated: &[PathBuf], path: &Path) -> io::Result<Gated> {
    let Some(folder) = gated_folder(gated, path) else {
        return safefs::open_dir_path(path).map(Gated::Opened);
    };
    match state(folder) {
        Some(Consent::Confirmed) => safefs::open_dir_path(path).map(Gated::Opened),
        Some(Consent::Pending) => Ok(Gated::Pending),
        None => {
            let path = path.to_path_buf();
            bounded(folder, move || safefs::open_dir_path(&path))
        }
    }
}

/// Run one open on a helper thread and wait [`CONSENT_WAIT`] for it. The
/// thread outlives a missed deadline: when the dialog is finally answered it
/// records the answer, and the descriptor it opened is closed unread.
///
/// Public because the dialog itself cannot be raised on purpose — raising
/// one is the defect — so the only honest test of this bound drives it with
/// an open that answers late.
pub fn bounded(
    folder: &Path,
    open: impl FnOnce() -> io::Result<OwnedFd> + Send + 'static,
) -> io::Result<Gated> {
    // One probe per folder per process: a question already with the person
    // at the keyboard is not asked again by a second thread.
    {
        let mut states = CONSENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if states.get(folder) == Some(&Consent::Pending) {
            return Ok(Gated::Pending);
        }
        states.insert(folder.to_path_buf(), Consent::Pending);
    }
    let (sender, receiver) = mpsc::channel();
    let recorded = folder.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("stado-consent-probe".to_string())
        .spawn(move || {
            let opened = open();
            set_state(
                &recorded,
                match &opened {
                    Ok(_) => Some(Consent::Confirmed),
                    // Refused, or the folder is not there: nothing pending,
                    // and the next open reports the error itself.
                    Err(_) => None,
                },
            );
            let _ = sender.send(opened);
        });
    if let Err(error) = spawned {
        set_state(folder, None);
        return Err(error);
    }
    match receiver.recv_timeout(CONSENT_WAIT) {
        Ok(opened) => opened.map(Gated::Opened),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(Gated::Pending),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(
            "the consent probe ended without an answer",
        )),
    }
}

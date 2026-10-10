//! One process per long-running role on a host.
//!
//! Three copies of a host's worker ran for weeks beside the installed one,
//! each started by hand from a build directory, and nothing in Stado refused
//! the second start or ended the extra ones: every start took the queue as
//! its own. A role now takes an exclusive kernel lock on
//! `~/.stado/roles/<role>.lock` before it does anything, writes who holds it
//! (pid, program, since) into the file, and keeps the lock for its life; a
//! second start reads that record and refuses at once, naming the holder.
//! The lock is the kernel's, so a holder that died releases it with no
//! stale-pid guessing. A role started from a program outside the installed
//! path (`~/.stado/bin`) — a repository's build directory, a copy of a
//! checkout — is refused too: an installed host runs the installed Stado,
//! and a stray build left running is the state this module exists to end.
//! `strays` lists the Stado role processes on this host that hold no role
//! lock or run a program that is gone, for the host-health pass to end.

mod strays;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

pub use strays::{end_strays, strays, Stray, StrayReason};

/// Where a host keeps its role locks, under the home directory.
const ROLES_DIRECTORY: &str = ".stado/roles";
/// Where the installed Stado programs live, under the home directory.
const INSTALLED_BIN: &str = ".stado/bin";

/// Who holds a role: written into the lock file by the holder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holder {
    pub role: String,
    pub pid: u32,
    pub program: PathBuf,
    pub since: String,
}

/// A role held for this process's life. Dropping it releases the lock.
pub struct Held {
    _file: File,
    pub holder: Holder,
}

/// The lock file of `role` under `home`.
pub fn lock_path(home: &Path, role: &str) -> PathBuf {
    home.join(ROLES_DIRECTORY).join(format!("{role}.lock"))
}

/// The installed Stado program under `home`, when one is installed.
pub fn installed_program(home: &Path) -> Option<PathBuf> {
    let program = home.join(INSTALLED_BIN).join("stado");
    program.is_file().then_some(program)
}

/// The role a `stado serve` invocation plays on its host: `serve` with the
/// target it serves, so one host serves once per target. A standalone
/// worker (no target) is the role `serve` alone.
pub fn serve_role(target: Option<&str>) -> String {
    match target {
        Some(target) => format!("serve--{target}"),
        None => "serve".to_string(),
    }
}

/// Take `role` for this process, or refuse naming who holds it.
///
/// `program` is this process's own executable. A program outside the
/// installed path is refused before the lock is tried, unless the home
/// holds no installed Stado at all (a development host that runs the
/// checkout's build directly has nothing to prefer).
pub fn take(home: &Path, role: &str, program: &Path) -> Result<Held, CmdError> {
    if let Some(installed) = installed_program(home) {
        if program != installed {
            return Err(CmdError::refused(format!(
                "role {role} is not started from the installed Stado: this program is {} and \
                 the installed one is {}; a role runs the installed program, so a build left \
                 running from a checkout can never hold a host's queue beside it (stado \
                 product install stado --surface cli installs a build)",
                program.display(),
                installed.display()
            )));
        }
    }
    let path = lock_path(home, role);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::unreachable(format!(
                "the role directory {} cannot be created: {error}",
                parent.display()
            ))
        })?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            CmdError::unreachable(format!(
                "the role lock {} cannot be opened: {error}",
                path.display()
            ))
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            return Err(refusal(role, &path));
        }
        Err(error) => {
            return Err(CmdError::unreachable(format!(
                "the role lock {} cannot be taken: {error}",
                path.display()
            )));
        }
    }
    let holder = Holder {
        role: role.to_string(),
        pid: std::process::id(),
        program: program.to_path_buf(),
        since: chrono::Utc::now().to_rfc3339(),
    };
    write_holder(&file, &holder, &path)?;
    Ok(Held {
        _file: file,
        holder,
    })
}

/// The holder recorded in `role`'s lock file, when a process holds it now.
/// `None` when nobody does: the file is absent, or its lock is free.
pub fn holder(home: &Path, role: &str) -> Result<Option<Holder>, CmdError> {
    let path = lock_path(home, role);
    let Ok(file) = File::open(&path) else {
        return Ok(None);
    };
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            Ok(None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            read_holder(&path).map(Some)
        }
        Err(error) => Err(CmdError::unreachable(format!(
            "the role lock {} cannot be read: {error}",
            path.display()
        ))),
    }
}

/// Every role held on this host now, with its holder.
pub fn holders(home: &Path) -> Result<Vec<Holder>, CmdError> {
    let directory = home.join(ROLES_DIRECTORY);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(Vec::new());
    };
    let mut held = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(role) = name.strip_suffix(".lock") else {
            continue;
        };
        if let Some(holder) = holder(home, role)? {
            held.push(holder);
        }
    }
    held.sort_by(|a, b| a.role.cmp(&b.role));
    Ok(held)
}

fn refusal(role: &str, path: &Path) -> CmdError {
    match read_holder(path) {
        Ok(holder) => CmdError::refused(format!(
            "role {role} already runs on this host: pid {}, program {}, since {}; one process \
             holds a role, and a second start of the same role is never admitted beside it \
             (stado service list shows every role's holder)",
            holder.pid,
            holder.program.display(),
            holder.since
        )),
        Err(error) => CmdError::refused(format!(
            "role {role} already runs on this host (its lock {} is held), and its holder record \
             could not be read: {error}",
            path.display()
        )),
    }
}

fn write_holder(mut file: &File, holder: &Holder, path: &Path) -> Result<(), CmdError> {
    let body = serde_json::to_vec_pretty(holder).map_err(|error| {
        CmdError::unreachable(format!("the role holder record cannot be written: {error}"))
    })?;
    let length = u64::try_from(body.len()).map_err(|error| {
        CmdError::unreachable(format!("the role holder record is too long: {error}"))
    })?;
    file.set_len(length)
        .and_then(|_| file.write_all(&body))
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            CmdError::unreachable(format!(
                "the role holder record {} cannot be written: {error}",
                path.display()
            ))
        })
}

fn read_holder(path: &Path) -> Result<Holder, CmdError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CmdError::unreachable(format!("{}: {error}", path.display())))?;
    serde_json::from_str(&text).map_err(|error| {
        CmdError::unreachable(format!(
            "{} does not hold a role holder record: {error}",
            path.display()
        ))
    })
}

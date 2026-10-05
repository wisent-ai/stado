//! The commands installed programs run of each other, observed as they run
//! them, so an installation never replaces a program with one that drops a
//! command an installed product still calls.
//!
//! Every `stado` invocation records, once per caller and command, which
//! executable started it and which command it ran. No list of callers or of
//! their commands is kept by hand: the record is what the programs did.
//! Replacing an installed program reads the records whose callee is that
//! program, keeps those whose caller is another product's installed path and
//! has not changed since it was recorded, and asks the candidate for each
//! recorded command with `--help`. A command the candidate does not answer
//! refuses the installation and names the product, its revision, the caller
//! and the command, so the dependent is updated first instead of breaking
//! after the swap.

use crate::common::{atomic_json, Runtime};
use crate::state;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

/// Where the records live, under HOME: one file per caller, callee and
/// command, named by the digest of the three.
const DIRECTORY: &str = ".stado/callers";

#[derive(Deserialize, Serialize)]
struct Call {
    caller: PathBuf,
    caller_modified: u128,
    caller_size: u64,
    callee: PathBuf,
    command: Vec<String>,
}

/// A file's modification time and size: the identity that tells a recorded
/// caller from a program that replaced it at the same path.
fn identity(path: &Path) -> Result<(u128, u64)> {
    let metadata = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    Ok((modified, metadata.len()))
}

#[cfg(target_os = "linux")]
fn executable_of(pid: u32) -> Result<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).with_context(|| format!("reading /proc/{pid}/exe"))
}

#[cfg(target_os = "macos")]
fn executable_of(pid: u32) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let pid = libc::c_int::try_from(pid).context("process id out of range")?;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the kernel writes at most `buffer.len()` bytes into this live
    // buffer and returns how many it wrote.
    let written =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 {
        bail!("proc_pidpath({pid}): {}", std::io::Error::last_os_error());
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn executable_of(_pid: u32) -> Result<PathBuf> {
    bail!("reading another process's executable is unsupported on this operating system")
}

/// Record that the process which started this one ran `command` of the
/// running program. A call already recorded for an unchanged caller writes
/// nothing, and so does a process whose effective user does not own HOME:
/// a `sudo stado` left a root-owned, owner-only record under the account's
/// HOME, and every later release install on that account then failed reading
/// it, with nothing but `Permission denied (os error 13)`.
pub fn record(command: &[String]) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    // SAFETY: geteuid has no preconditions and cannot fail.
    let effective = unsafe { libc::geteuid() };
    if fs::metadata(&home).is_ok_and(|metadata| metadata.uid() != effective) {
        return Ok(());
    }
    let callee = std::env::current_exe()?.canonicalize()?;
    let caller = executable_of(std::os::unix::process::parent_id())?;
    let caller = caller.canonicalize().unwrap_or(caller);
    if caller == callee {
        return Ok(());
    }
    let (caller_modified, caller_size) = identity(&caller)?;
    let key = format!(
        "{}\0{}\0{caller_modified}\0{caller_size}\0{}",
        caller.display(),
        callee.display(),
        command.join("\0")
    );
    let path = home.join(DIRECTORY).join(format!(
        "{}.json",
        hex::encode(Sha256::digest(key.as_bytes()))
    ));
    if path.exists() {
        return Ok(());
    }
    let call = Call {
        caller,
        caller_modified,
        caller_size,
        callee,
        command: command.to_vec(),
    };
    atomic_json(&path, &serde_json::to_value(call)?)
}

/// Refuse placing `candidate` at `destination` when an installed product was
/// recorded running a command of the program at `destination` that
/// `candidate` does not answer.
pub fn refuse_removed(runtime: &Runtime, destination: &Path, candidate: &Path) -> Result<()> {
    let directory = runtime.home.join(DIRECTORY);
    if !directory.is_dir() {
        return Ok(());
    }
    let destination = destination
        .canonicalize()
        .unwrap_or_else(|_| destination.to_path_buf());
    let mut installed = Vec::new();
    for product in state::all(runtime)? {
        for path in &product.installed_paths {
            installed.push((
                path.canonicalize().unwrap_or_else(|_| path.clone()),
                product.clone(),
            ));
        }
    }
    let mut missing = Vec::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("reading caller records in {}", directory.display()))?
    {
        let path = entry?.path();
        // A record this account cannot read was not written by this account's
        // own programs (a root-run Stado writes owner-only files), so it says
        // nothing about what they run; it is named and left out of the check
        // instead of refusing the installation with a bare permission error.
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "caller record {} cannot be read by this account ({error}); it was not \
                     written by this account's programs and is left out of the check",
                    path.display()
                );
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading caller record {}", path.display()))
            }
        };
        let call: Call = serde_json::from_slice(&bytes)
            .with_context(|| format!("reading caller record {}", path.display()))?;
        if call.callee != destination {
            continue;
        }
        // A caller replaced or removed since it was recorded no longer proves
        // what the installed program runs; its record goes.
        let unchanged = identity(&call.caller)
            .is_ok_and(|current| current == (call.caller_modified, call.caller_size));
        if !unchanged {
            fs::remove_file(&path)
                .with_context(|| format!("removing stale caller record {}", path.display()))?;
            continue;
        }
        let Some((_, owner)) = installed.iter().find(|(owned, _)| *owned == call.caller) else {
            continue;
        };
        let answered = Command::new(candidate)
            .args(&call.command)
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| {
                format!(
                    "asking {} for `{} --help`",
                    candidate.display(),
                    call.command.join(" ")
                )
            })?;
        if !answered.success() {
            missing.push(format!(
                "{} {} (installed at {}, source revision {}) runs `{} {}`",
                owner.product,
                owner.surface,
                call.caller.display(),
                owner.source_revision.as_deref().unwrap_or("unrecorded"),
                destination.display(),
                call.command.join(" ")
            ));
        }
    }
    if !missing.is_empty() {
        bail!(
            "{} does not answer commands installed products still run, so it would break them; \
             update these products to a revision that no longer runs them, then install again: {}",
            candidate.display(),
            missing.join("; ")
        );
    }
    Ok(())
}

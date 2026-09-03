//! One agent per host, enforced rather than assumed.
//!
//! # The defect this exists for
//!
//! Nothing prevented two agents from running on one host, and on 2026-09-03
//! `charless-mac-mini` had several: live `~/.stado/bin/stado` processes with
//! `ppid = 1`, ten hours of uptime and no launchd label, left behind by a day
//! of service switches and binary swaps. They contend for the janitor's
//! exclusive run lock, so the host reported `lock_busy` on every tick, never
//! recorded a completed pass, and `deploy::host_gates` refused claiming for
//! the whole fleet on the strength of it. The product never said "two agents
//! are running here"; it only said the lock was busy, which reads like
//! ordinary contention with the agent's own tick.
//!
//! # Why a lock and not a pid file
//!
//! A pid file records an intention; a lock records a fact, and the kernel
//! withdraws it the instant the holder exits, including when it is killed.
//! This is the discipline `disk_cleanup` already uses, on its own file, so
//! nothing here can interfere with the cleanup lock's meaning.
//!
//! # Why the NEWCOMER loses
//!
//! The live holder keeps the lock and the starting process refuses. A supervisor
//! restart, a stray `stado agent` in a terminal, or a second unit therefore
//! cannot displace an agent that is working; and the refusal is a named error
//! on the way out rather than two processes taking turns losing a lock. The
//! orphan case is not solved by this alone -- an orphan that still works keeps
//! the host -- but it becomes visible in one line instead of three samples of
//! `netstat`.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

/// The lock file, beside the janitor's, under `~/.cache/wisent-compute`.
const AGENT_LOCK_NAME: &str = "stado-agent.lock";

/// An exclusive hold for the lifetime of this agent process.
pub struct AgentSingleton {
    file: File,
}

impl Drop for AgentSingleton {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn lock_path() -> PathBuf {
    let home = crate::config_file::expand_tilde("~");
    home.join(".cache").join("wisent-compute").join(AGENT_LOCK_NAME)
}

/// Take the host's agent lock. `None` means another agent holds it.
///
/// # Why the criterion is the lock and never the file
///
/// A guard that refused on the EXISTENCE of a marker would be the trap this
/// fleet has already been caught in twice today: a marker that outlives its
/// owner closes the host for good, and the only process that could clear it is
/// dead. `flock` cannot do that. The kernel holds it on behalf of a live
/// process and withdraws it the instant that process exits, however it exits,
/// so holding it IS the proof of life and a leftover file is simply an
/// unlocked file that the next agent locks and starts. There is nothing to
/// clear by hand and no state that can strand a host.
///
/// Checked against this fleet's current mess rather than argued: on
/// 2026-09-03 `charless-mac-mini` carried three `ppid = 1` stado processes
/// from builds that predate this file and therefore hold no agent lock at
/// all, so a real agent starting there today takes the lock and runs. The
/// refusal can only ever be produced by a process that is alive now.
///
/// The pid is recorded in the file's CONTENTS purely so the refusal can name
/// the holder. It is never the criterion: a stale pid in there decides
/// nothing.
pub fn hold(log_fn: &mut impl FnMut(&str)) -> std::io::Result<Option<AgentSingleton>> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != unsafe { nix::libc::geteuid() } {
        return Err(std::io::Error::other(format!(
            "unsafe agent lock at {}",
            path.display()
        )));
    }
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            use std::io::{Seek, SeekFrom, Write};
            let _ = file.set_len(0);
            let _ = file.seek(SeekFrom::Start(0));
            let _ = writeln!(
                file,
                "{} {}",
                std::process::id(),
                env!("CARGO_PKG_VERSION")
            );
            log_fn(&format!(
                "init: holding this host's agent lock at {}; a second agent will refuse to start \
                 while this process lives",
                path.display()
            ));
            Ok(Some(AgentSingleton { file }))
        }
        Err(exc)
            if exc.kind() == std::io::ErrorKind::WouldBlock
                || matches!(exc.raw_os_error(), Some(code) if code == nix::libc::EACCES
                    || code == nix::libc::EAGAIN) =>
        {
            let holder = std::fs::read_to_string(&path)
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            let holder = if holder.is_empty() {
                "an agent that recorded no pid".to_string()
            } else {
                format!("pid/version {holder}")
            };
            log_fn(&format!(
                "init: {} holds a LIVE lock on {} -- the kernel would have released it had that \
                 process exited -- so this host already has an agent and starting a second one \
                 would only make both of them lose the janitor run lock",
                holder,
                path.display()
            ));
            Ok(None)
        }
        Err(exc) => Err(exc),
    }
}

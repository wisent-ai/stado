//! The editor connection belongs to its parent, even if another process keeps stdin open.
use anyhow::{bail, Context, Result};

#[cfg(target_os = "macos")]
pub struct Parent {
    pub pid: libc::pid_t,
    queue: std::sync::Arc<std::os::fd::OwnedFd>,
}

#[cfg(target_os = "macos")]
fn event(
    parent: libc::pid_t,
    filter: libc::c_short,
    flags: libc::c_ushort,
    fflags: libc::c_uint,
) -> libc::kevent {
    libc::kevent {
        ident: parent as libc::uintptr_t,
        filter,
        flags,
        fflags,
        data: Default::default(),
        udata: std::ptr::null_mut(),
    }
}

#[cfg(target_os = "macos")]
impl Parent {
    pub fn observe() -> Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::sync::Arc;
        let pid = unsafe { libc::getppid() };
        let descriptor = unsafe { libc::kqueue() };
        if descriptor.is_negative() {
            return Err(std::io::Error::last_os_error())
                .context("create editor parent event queue");
        }
        let queue = Arc::new(unsafe { OwnedFd::from_raw_fd(descriptor) });
        let changes = [
            event(
                pid,
                libc::EVFILT_PROC,
                libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
                libc::NOTE_EXIT,
            ),
            event(
                pid,
                libc::EVFILT_USER,
                libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
                Default::default(),
            ),
        ];
        let registered = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                changes.as_ptr(),
                changes.len().try_into()?,
                std::ptr::null_mut(),
                Default::default(),
                std::ptr::null(),
            )
        };
        if registered.is_negative() {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("observe editor parent {pid}"));
        }
        if unsafe { libc::getppid() } != pid {
            bail!("editor parent {pid} exited while its lifetime observer was registered");
        }
        let watched = Arc::clone(&queue);
        std::thread::Builder::new()
            .name("editor-parent".into())
            .spawn(move || {
                let mut observed = [event(
                    pid,
                    libc::EVFILT_USER,
                    Default::default(),
                    Default::default(),
                )];
                loop {
                    let result = unsafe {
                        libc::kevent(
                            watched.as_raw_fd(),
                            std::ptr::null(),
                            Default::default(),
                            observed.as_mut_ptr(),
                            observed.len() as libc::c_int,
                            std::ptr::null(),
                        )
                    };
                    if result.is_negative() {
                        let cause = std::io::Error::last_os_error();
                        if cause.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        eprintln!("editor parent {pid} observation failed: {cause}");
                        std::process::exit(libc::EXIT_FAILURE);
                    }
                    if observed
                        .iter()
                        .any(|event| event.filter == libc::EVFILT_PROC)
                    {
                        eprintln!("editor parent {pid} exited; closing the editor server");
                        std::process::exit(libc::EXIT_SUCCESS);
                    }
                    return;
                }
            })
            .context("start editor parent event observer")?;
        Ok(Self { pid, queue })
    }
}

#[cfg(target_os = "macos")]
impl Drop for Parent {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let changes = [event(
            self.pid,
            libc::EVFILT_USER,
            Default::default(),
            libc::NOTE_TRIGGER,
        )];
        let result = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as libc::c_int,
                std::ptr::null_mut(),
                Default::default(),
                std::ptr::null(),
            )
        };
        if result.is_negative() {
            eprintln!(
                "cancel editor parent observer: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub struct Parent {
    pub pid: libc::pid_t,
}

#[cfg(target_os = "linux")]
impl Parent {
    pub fn observe() -> Result<Self> {
        let pid = unsafe { libc::getppid() };
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) }.is_negative() {
            return Err(std::io::Error::last_os_error())
                .context("bind editor lifetime to parent exit");
        }
        if unsafe { libc::getppid() } != pid {
            bail!("editor parent {pid} exited while its lifetime observer was registered");
        }
        Ok(Self { pid })
    }
}

#[cfg(target_os = "linux")]
impl Drop for Parent {
    fn drop(&mut self) {
        let disabled: libc::c_int = Default::default();
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, disabled) }.is_negative() {
            eprintln!(
                "cancel editor parent observer: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Parent {
    pub fn observe() -> Result<Self> {
        bail!(
            "editor parent lifetime observation is unsupported on {}",
            std::env::consts::OS
        )
    }
}

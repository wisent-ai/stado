use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use nix::fcntl::{fcntl, FcntlArg, FdFlag, OFlag};
use nix::sys::socket::{connect, socket, AddressFamily, SockFlag, SockType, UnixAddr};

pub(crate) struct Prepared {
    pub(super) listener: UnixListener,
    pub(super) guard: SocketGuard,
}

/// Why the release proxy's control socket could not be prepared: an
/// operating-system failure with the step it met, a path this process will
/// not take over, or a socket location it cannot derive.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlSocketError {
    #[error("{context}: {error}")]
    Io { context: String, error: std::io::Error },
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Config(String),
}

fn io(context: String) -> impl FnOnce(std::io::Error) -> ControlSocketError {
    move |error| ControlSocketError::Io { context, error }
}

fn native(context: String) -> impl FnOnce(nix::errno::Errno) -> ControlSocketError {
    move |errno| ControlSocketError::Io {
        context,
        error: std::io::Error::from(errno),
    }
}

pub(super) struct SocketGuard {
    _lock: File,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
        {
            if let Err(error) = fs::remove_file(&self.path) {
                eprintln!(
                    "cannot remove owned proxy control socket {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

// A nonblocking native connect distinguishes a crashed owner from an unrelated
// live listener. Holding our separate file lock is not permission to unlink a
// socket belonging to a program that does not participate in that lock.
fn retire_stale_socket(path: &Path, uid: u32) -> Result<(), ControlSocketError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io(format!("cannot inspect control path {}", path.display()))(error))
        }
    };
    if !metadata.file_type().is_socket() || metadata.uid() != uid {
        return Err(ControlSocketError::Refused(format!(
            "refusing unrelated proxy control path {}",
            path.display()
        )));
    }
    let probe = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )
    .map_err(native("cannot create native control-socket probe".to_string()))?;
    fcntl(&probe, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
        .and_then(|_| fcntl(&probe, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)))
        .map_err(native("cannot configure native control-socket probe".to_string()))?;
    let address = UnixAddr::new(path).map_err(|error| {
        ControlSocketError::Config(format!("invalid control socket path {}: {error}", path.display()))
    })?;
    match connect(probe.as_raw_fd(), &address) {
        Err(nix::errno::Errno::ECONNREFUSED) => {
            let current = fs::symlink_metadata(path)
                .map_err(io("cannot recheck stale control socket".to_string()))?;
            if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                return Err(ControlSocketError::Refused(
                    "control socket changed while its owner was inspected".to_string(),
                ));
            }
            fs::remove_file(path)
                .map_err(io(format!("cannot remove stale control socket {}", path.display())))
        }
        Err(nix::errno::Errno::ENOENT) => Ok(()),
        result => Err(ControlSocketError::Refused(format!(
            "refusing control socket {}: native connection did not prove its owner absent ({result:?})",
            path.display()
        ))),
    }
}

pub(crate) fn prepare() -> Result<Prepared, ControlSocketError> {
    let path = super::socket_path(None).map_err(ControlSocketError::Config)?;
    let parent = path.parent().ok_or_else(|| {
        ControlSocketError::Config("control socket has no parent directory".to_string())
    })?;
    fs::create_dir_all(parent)
        .map_err(io(format!("cannot create control directory {}", parent.display())))?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(io(format!("cannot open proxy owner lock {}", lock_path.display())))?;
    let uid = nix::unistd::geteuid().as_raw();
    let metadata = lock
        .metadata()
        .map_err(io("cannot inspect proxy owner lock".to_string()))?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0 {
        return Err(ControlSocketError::Refused(format!(
            "proxy owner lock is not this user's owner-only file: {}",
            lock_path.display()
        )));
    }
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(io(format!("cannot acquire proxy owner lock {}", lock_path.display())))?;
    retire_stale_socket(&path, uid)?;
    let listener = UnixListener::bind(&path)
        .map_err(io(format!("cannot bind proxy control socket {}", path.display())))?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(io("cannot inspect bound control socket".to_string()))?;
    let guard = SocketGuard {
        _lock: lock,
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        ControlSocketError::Io {
            context: format!("cannot protect proxy control socket {}", path.display()),
            error,
        }
    })?;
    listener
        .set_nonblocking(true)
        .map_err(io("cannot configure proxy control listener".to_string()))?;
    eprintln!(
        "stado release proxy owner: pid={} socket={}",
        std::process::id(),
        path.display()
    );
    Ok(Prepared { listener, guard })
}

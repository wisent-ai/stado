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
fn retire_stale_socket(path: &Path, uid: u32) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot inspect control path {}: {error}",
                path.display()
            ))
        }
    };
    if !metadata.file_type().is_socket() || metadata.uid() != uid {
        return Err(format!(
            "refusing unrelated proxy control path {}",
            path.display()
        ));
    }
    let probe = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )
    .map_err(|error| format!("cannot create native control-socket probe: {error}"))?;
    fcntl(&probe, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
        .and_then(|_| fcntl(&probe, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)))
        .map_err(|error| format!("cannot configure native control-socket probe: {error}"))?;
    let address = UnixAddr::new(path)
        .map_err(|error| format!("invalid control socket path {}: {error}", path.display()))?;
    match connect(probe.as_raw_fd(), &address) {
        Err(nix::errno::Errno::ECONNREFUSED) => {
            let current = fs::symlink_metadata(path)
                .map_err(|error| format!("cannot recheck stale control socket: {error}"))?;
            if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                return Err("control socket changed while its owner was inspected".to_string());
            }
            fs::remove_file(path)
                .map_err(|error| format!("cannot remove stale control socket {}: {error}", path.display()))
        }
        Err(nix::errno::Errno::ENOENT) => Ok(()),
        result => Err(format!(
            "refusing control socket {}: native connection did not prove its owner absent ({result:?})",
            path.display()
        )),
    }
}

pub(crate) fn prepare() -> Result<Prepared, String> {
    let path = super::socket_path(None)?;
    let parent = path
        .parent()
        .ok_or_else(|| "control socket has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create control directory {}: {error}",
            parent.display()
        )
    })?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot open proxy owner lock {}: {error}",
                lock_path.display()
            )
        })?;
    let uid = nix::unistd::geteuid().as_raw();
    let metadata = lock
        .metadata()
        .map_err(|error| format!("cannot inspect proxy owner lock: {error}"))?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "proxy owner lock is not this user's owner-only file: {}",
            lock_path.display()
        ));
    }
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|error| {
        format!(
            "cannot acquire proxy owner lock {}: {error}",
            lock_path.display()
        )
    })?;
    retire_stale_socket(&path, uid)?;
    let listener = UnixListener::bind(&path).map_err(|error| {
        format!(
            "cannot bind proxy control socket {}: {error}",
            path.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect bound control socket: {error}"))?;
    let guard = SocketGuard {
        _lock: lock,
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "cannot protect proxy control socket {}: {error}",
            path.display()
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure proxy control listener: {error}"))?;
    eprintln!(
        "stado release proxy owner: pid={} socket={}",
        std::process::id(),
        path.display()
    );
    Ok(Prepared { listener, guard })
}

//! `stado space file remove-local PATH --home HOME`: the host-side half of
//! `stado space file remove`, run by the installed Stado on the target (under
//! `sudo -n` for the two declared privileged unit locations).
//!
//! Like retirement it holds directory descriptors and never follows a path
//! component: every parent is opened with `O_NOFOLLOW`, the file is inspected
//! with `AT_SYMLINK_NOFOLLOW` and unlinked through the held parent. It prints
//! one `STADO_REMOVE_FILE\t[status, detail]` line, the report `remove.rs`
//! reads back; statuses are `removed`, `absent`, `refused` and `failed`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::libc;

/// Where a user-owned file may be removed from, relative to HOME.
const USER_ROOTS: [&str; 3] = ["Library/LaunchAgents", ".stado", ".config/systemd/user"];
/// The unit prefix Stado owns in the two privileged locations.
const UNIT_PREFIX: &str = "com.wisent.";

fn report(status: &str, detail: &str) {
    println!(
        "STADO_REMOVE_FILE\t{}",
        serde_json::json!([status, detail])
    );
}

fn privileged(parent: &str, name: &str) -> bool {
    name.starts_with(UNIT_PREFIX)
        && ((parent == "/Library/LaunchDaemons" && name.ends_with(".plist"))
            || (parent == "/etc/systemd/system" && name.ends_with(".service")))
}

fn c_name(text: &str) -> Option<CString> {
    CString::new(text.as_bytes()).ok()
}

/// Walk from `/` to `parent`, one `O_NOFOLLOW` directory at a time.
fn open_parent(parent: &str) -> Result<libc::c_int, (bool, std::io::Error)> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let root = c_name("/").expect("no NUL");
    let mut descriptor = unsafe { libc::open(root.as_ptr(), flags) };
    if descriptor < 0 {
        return Err((false, std::io::Error::last_os_error()));
    }
    for component in parent.split('/').filter(|piece| !piece.is_empty()) {
        let name = c_name(component).ok_or((false, std::io::Error::from_raw_os_error(libc::EINVAL)))?;
        let next = unsafe { libc::openat(descriptor, name.as_ptr(), flags) };
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(descriptor) };
        if next < 0 {
            return Err((true, error));
        }
        descriptor = next;
    }
    Ok(descriptor)
}

fn stat_at(descriptor: libc::c_int, name: &CString) -> std::io::Result<libc::stat> {
    let mut found = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(descriptor, name.as_ptr(), found.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW)
    };
    if result == 0 {
        Ok(unsafe { found.assume_init() })
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn not_found(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOENT)
}

fn link_or_file(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR))
}

/// The status and detail of one guarded removal.
fn remove(path: &str, home: &str) -> (&'static str, String) {
    let home = home.trim_end_matches('/');
    let user_owned = USER_ROOTS
        .iter()
        .any(|root| path.starts_with(&format!("{home}/{root}/")));
    let target = Path::new(path);
    let parent = target.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let privileged = privileged(&parent, &name);
    if !user_owned && !privileged {
        return ("refused", "outside the managed areas".into());
    }
    let euid = unsafe { libc::geteuid() };
    if privileged && euid != 0 {
        return (
            "refused",
            "the declared privileged file operation requires the target's sudo authority".into(),
        );
    }
    let Some(c_file) = c_name(&name).filter(|_| !name.is_empty()) else {
        return ("refused", "the path names no file".into());
    };
    let descriptor = match open_parent(&parent) {
        Ok(descriptor) => descriptor,
        Err((_, error)) if not_found(&error) => return ("absent", String::new()),
        Err((_, error)) if link_or_file(&error) => {
            return (
                "refused",
                format!("open parent directory without following links: a parent is a symbolic link or is not a directory: {error}"),
            )
        }
        Err((_, error)) => {
            return ("failed", format!("open parent directory without following links: {error}"))
        }
    };
    let outcome = (|| {
        let before = match stat_at(descriptor, &c_file) {
            Ok(found) => found,
            Err(error) if not_found(&error) => return ("absent", String::new()),
            Err(error) => {
                return ("failed", format!("inspect the selected file without following links: {error}"))
            }
        };
        let kind = before.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK {
            return (
                "refused",
                "a symbolic link is not removed by the managed regular-file operation".into(),
            );
        }
        if kind != libc::S_IFREG {
            return ("refused", "not a regular file".into());
        }
        if user_owned && before.st_uid != euid {
            return ("refused", "not owned by this account".into());
        }
        if unsafe { libc::unlinkat(descriptor, c_file.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            if not_found(&error) {
                return ("absent", String::new());
            }
            return (
                "failed",
                format!("unlink the selected file through its held parent directory: {error}"),
            );
        }
        match stat_at(descriptor, &c_file) {
            Err(error) if not_found(&error) => ("removed", String::new()),
            Err(error) => (
                "failed",
                format!("verify file absence through the held parent directory: {error}"),
            ),
            Ok(_) => ("failed", "the selected file was recreated after removal".into()),
        }
    })();
    unsafe { libc::close(descriptor) };
    outcome
}

/// Print the one report line for PATH under HOME. Always succeeds as a
/// process: the report carries the outcome, as the host channel expects.
pub fn remove_file_local(path: &str, home: &str) {
    let (status, detail) = remove(path, home);
    report(status, &detail);
}

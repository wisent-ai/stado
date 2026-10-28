//! The swap itself: put one verified file over the installed one atomically,
//! then replace this process image with the binary that was just installed.

use std::path::Path;

use crate::self_update::verify::targets::current_exe_path;
use crate::self_update::SelfUpdateError;

/// Put the verified staging file over `dest` whole and durably
/// ([`crate::primitives::document::persist_copy`]): copied into a fresh file
/// beside it with the program mode, flushed, renamed over it (atomic on the
/// same filesystem) and the directory flushed, so a host that loses power
/// after the swap starts the new binary, not a missing or partial one. A
/// failed swap removes its staging file; the old binary at `dest` is
/// untouched.
pub(in crate::self_update) fn replace_verified(
    staged: &Path,
    dest: &Path,
) -> Result<(), SelfUpdateError> {
    if dest.file_name().is_none() {
        return Err(SelfUpdateError::NoInstallDir(dest.to_path_buf()));
    }
    crate::primitives::document::persist_copy(
        dest,
        staged,
        crate::primitives::file_mode::program(),
    )?;
    Ok(())
}

/// Re-exec the (just-replaced) current binary with the original argv,
/// inheriting the current environment — Python's `os.execv(path, argv)`
/// semantics. NEVER returns on success: the process image is replaced.
/// The returned error means the exec failed and the caller should keep
/// running the old in-memory binary.
pub fn reexec() -> std::io::Error {
    use std::os::unix::process::CommandExt;
    // The first argument is this program's own name; the exec names the
    // binary itself, so only the rest is handed on.
    let mut arguments = std::env::args_os();
    arguments.next();
    match current_exe_path() {
        Ok(exe) => std::process::Command::new(exe).args(arguments).exec(),
        Err(err) => err,
    }
}

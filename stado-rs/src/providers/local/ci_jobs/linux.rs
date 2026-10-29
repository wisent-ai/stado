//! Process names on Linux: `/proc/<pid>/comm`, the kernel's own record of
//! each process's executable name (proc(5)).

use std::path::Path;

const PROC: &str = "/proc";

/// The executable name of every process the table lists. A process that
/// ends between the listing and the read is skipped; any other failure is
/// the error, so an unreadable table is never read as no CI.
pub(super) fn process_names() -> Result<Vec<String>, String> {
    let entries =
        std::fs::read_dir(PROC).map_err(|error| format!("cannot list {PROC}: {error}"))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read an entry of {PROC}: {error}"))?;
        let pid = entry.file_name();
        if !pid
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let comm = Path::new(PROC).join(&pid).join("comm");
        match std::fs::read_to_string(&comm) {
            Ok(name) => names.push(name.trim_end_matches('\n').to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot read {}: {error}", comm.display())),
        }
    }
    Ok(names)
}

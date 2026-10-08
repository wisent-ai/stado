//! Export exactly the requested commit and its reachable history, without
//! changing a checkout or publishing unrelated branches and tags.

use std::path::Path;

use crate::cli::CmdError;

use super::git;

pub(super) fn export(
    source: &Path,
    commit: &str,
    scratch: &Path,
) -> Result<tempfile::NamedTempFile, CmdError> {
    std::fs::create_dir_all(scratch)?;
    let archive = tempfile::NamedTempFile::new_in(scratch)?;
    let reference = format!("refs/stado/release-input/{}", uuid::Uuid::new_v4().simple());
    // A bundle needs an advertised ref, not just an object ID. The private
    // ref is unique to this invocation; an empty expected old value makes
    // creation refuse an existing ref rather than replacing it.
    git(source, &["update-ref", &reference, commit, ""])?;
    let result = git(
        source,
        &[
            "bundle",
            "create",
            &archive.path().to_string_lossy(),
            &reference,
        ],
    );
    let cleanup = git(source, &["update-ref", "-d", &reference, commit]);
    match (result, cleanup) {
        (Ok(_), Ok(_)) => Ok(archive),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(CmdError::click(format!(
            "Git bundle was created but its temporary reference {reference} could not be removed: {error}; no input was published"
        ))),
        (Err(error), Err(cleanup)) => Err(CmdError::click(format!(
            "Git bundle export failed: {error}; temporary reference {reference} cleanup also failed: {cleanup}; no input was published"
        ))),
    }
}

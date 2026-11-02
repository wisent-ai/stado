//! `--revision`: build a committed revision of the manifest's repository
//! instead of its working tree, so edits not yet committed - anyone's
//! half-written change in a shared checkout - never enter the build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| format!("git could not run in {}: {error}; install Git", directory.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git {} in {} exited {}: {}",
            arguments.join(" "),
            directory.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The committed tree of `revision` of the repository holding `package_dir`,
/// exported once under `area/revisions/<commit>`; the package's directory in
/// it and the full commit.
pub fn export(package_dir: &Path, revision: &str, area: &Path) -> Result<(PathBuf, String), String> {
    let top = PathBuf::from(git(package_dir, &["rev-parse", "--show-toplevel"]).map_err(|why| {
        format!("--revision needs the manifest inside a Git repository: {why}")
    })?);
    let commit = git(&top, &["rev-parse", "--verify", "--quiet", &format!("{revision}^{{commit}}")]).map_err(|_| {
        format!("{revision} names no commit of {}; fetch it, or name a commit, branch or tag the repository holds", top.display())
    })?;
    let relative = package_dir
        .strip_prefix(&top)
        .map_err(|_| format!("{} is not under its repository {}", package_dir.display(), top.display()))?
        .to_path_buf();
    let export = area.join("revisions").join(&commit);
    if !export.join(".exported").exists() {
        let _ = std::fs::remove_dir_all(&export);
        std::fs::create_dir_all(&export).map_err(|error| format!("{} cannot be made: {error}", export.display()))?;
        let archive = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(["archive", "--format=tar", &commit])
            .output()
            .map_err(|error| format!("git archive could not run in {}: {error}", top.display()))?;
        if !archive.status.success() {
            return Err(format!(
                "git archive {commit} in {} exited {}: {}",
                top.display(),
                archive.status,
                String::from_utf8_lossy(&archive.stderr).trim()
            ));
        }
        let tar = export.join(".archive.tar");
        std::fs::write(&tar, &archive.stdout).map_err(|error| format!("{} cannot be written: {error}", tar.display()))?;
        let unpacked = Command::new("tar")
            .arg("-xf")
            .arg(&tar)
            .arg("-C")
            .arg(&export)
            .output()
            .map_err(|error| format!("tar could not run to unpack {}: {error}", tar.display()))?;
        let _ = std::fs::remove_file(&tar);
        if !unpacked.status.success() {
            return Err(format!(
                "{commit} did not unpack into {} ({}): {}",
                export.display(),
                unpacked.status,
                String::from_utf8_lossy(&unpacked.stderr).trim()
            ));
        }
        std::fs::write(export.join(".exported"), &commit)
            .map_err(|error| format!("{} cannot be marked exported: {error}", export.display()))?;
    }
    Ok((export.join(relative), commit))
}

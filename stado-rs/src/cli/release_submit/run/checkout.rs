//! Source identity: the commit a submission is made of, what the checkout
//! holds beyond it, and the committed tree read out of git.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use flate2::{Compression, GzBuilder};

use crate::cli::CmdError;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, CmdError> {
    let o = crate::wait::output(
        Command::new("git")
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(root),
    )?;
    if !o.status.success() {
        return Err(CmdError::click(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(o.stdout)
}

/// `git` output as text.
pub(crate) fn git_text(root: &Path, args: &[&str]) -> Result<String, CmdError> {
    String::from_utf8(git(root, args)?).map_err(|_| {
        CmdError::click(format!("git {} answered non-UTF-8", args.join(" ")))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
/// The directory Stado itself writes inside a checkout (install builds,
/// quality scratch, the packager's `WISENT_OUTPUT_DIR`). Its contents are
/// Stado's own output, never an operator change, so a checkout holding it is
/// still clean.
const STADO_OUTPUT_DIR: &str = ".wisent-output/";

pub(crate) fn resolve_commit(root: &Path, requested: Option<&str>) -> Result<String, CmdError> {
    let commit = match requested {
        Some(commit) => commit.to_owned(),
        None => {
            let changed: Vec<String> = uncommitted_paths(root)?
                .into_iter()
                .filter(|path| !path.starts_with(STADO_OUTPUT_DIR))
                .collect();
            if !changed.is_empty() {
                return Err(CmdError::refused(format!(
                    "release source must be a clean committed Git tree; uncommitted: {}",
                    changed.join(", ")
                )));
            }
            head_commit(root)?
        }
    };
    let lowercase_hex = !commit.is_empty()
        && commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !lowercase_hex {
        return Err(CmdError::usage(
            "--commit must be a lowercase hexadecimal Git commit id, full or abbreviated",
        ));
    }
    // Git decides what the id names: a full id or an abbreviation of exactly
    // one commit resolves to the full commit id; an abbreviation matching no
    // commit or several objects, or an object that is no commit, is refused
    // with Git's own reason, so the caller sees whether to give more digits
    // or another id.
    let resolved = crate::wait::output(
        Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{commit}^{{commit}}"),
            ])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(root),
    )?;
    if !resolved.status.success() {
        return Err(CmdError::usage(format!(
            "--commit {commit} names no single commit in {}: {}",
            root.display(),
            String::from_utf8_lossy(&resolved.stderr).trim()
        )));
    }
    let full = String::from_utf8(resolved.stdout).map_err(|_| {
        CmdError::click("git rev-parse printed a commit id that is not UTF-8")
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    Ok(full.trim().to_owned())
}

/// The commit the checkout stands on, checked to be a full commit object.
pub(crate) fn head_commit(root: &Path) -> Result<String, CmdError> {
    let head = String::from_utf8(git(root, &["rev-parse", "HEAD"])?).map_err(|_| {
        CmdError::click("Git commit is not UTF-8")
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    resolve_commit(root, Some(head.trim()))
}

/// Every path the checkout holds that its HEAD commit does not - modified,
/// added, deleted, renamed from, renamed to and untracked - relative to the
/// repository root, which is what porcelain status always reports.
///
/// A release is made of commit objects alone (`snapshot`, `committed_file`),
/// so none of these paths reaches a build. What one of them can do is make
/// the commit say something the operator no longer means, such as an
/// uncommitted version bump; the caller decides which paths carry that.
pub(crate) fn uncommitted_paths(root: &Path) -> Result<BTreeSet<String>, CmdError> {
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?;
    let mut paths = BTreeSet::new();
    let mut entries = status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let (code, path) = match (entry.get(..2), entry.get(3..)) {
            (Some(code), Some(path)) if !path.is_empty() => (code, path),
            _ => {
                return Err(CmdError::click(format!(
                    "git status printed an entry this reader does not know: {:?}",
                    String::from_utf8_lossy(entry)
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown))
            }
        };
        paths.insert(String::from_utf8_lossy(path).into_owned());
        if code.iter().any(|status| matches!(status, b'R' | b'C')) {
            let origin = entries.next().ok_or_else(|| {
                CmdError::click(format!(
                    "git status named a rename of {} without the path it came from",
                    String::from_utf8_lossy(path)
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            paths.insert(String::from_utf8_lossy(origin).into_owned());
        }
    }
    Ok(paths)
}

pub(crate) fn committed_file(root: &Path, commit: &str, path: &str) -> Result<Vec<u8>, CmdError> {
    git(root, &["show", &format!("{commit}:{path}")])
}

/// The committed tree as one gzip tar of its regular files.
///
/// `git archive` also writes one entry per directory and a pax global header
/// naming the commit. Neither carries anything (Git tracks no empty
/// directory, every extractor creates a file's parents, and the commit is
/// recorded on the object and in the run), but each counts against the
/// workers' archive entry bound, so a snapshot is its files alone. Anything
/// that is not a file or a directory - a symlink, a device - is refused here,
/// as every extractor refuses it, rather than dropped on the way.
pub(crate) fn snapshot(root: &Path, commit: &str) -> Result<Vec<u8>, CmdError> {
    let tar = git(root, &["archive", "--format=tar", commit])?;
    let mut gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    {
        let mut files = tar::Builder::new(&mut gz);
        let mut archive = tar::Archive::new(&tar[..]);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if kind.is_dir() || kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                continue;
            }
            let path = entry.path()?.into_owned();
            if !kind.is_file() {
                return Err(CmdError::refused(format!(
                    "the committed tree carries {}, which is not a regular file; a source \
                     snapshot is made of files only",
                    path.display()
                )));
            }
            let mut header = entry.header().clone();
            files.append_data(&mut header, &path, &mut entry)?;
        }
        super::provenance::append_records(&mut files, root, commit)?;
        files.finish()?;
    }
    Ok(gz.finish()?)
}

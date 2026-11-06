//! The catalog file in a checkout's working tree, beside the committed one a
//! reader read: which checkout tracks it, what an uncommitted edit of it is
//! said to be, and what an edit's answer says about who sees it.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::Provenance;
use crate::common::{capture, git_command};
use crate::source::git;

/// The checkout that tracks the catalog file `file` and the file's path in
/// it, or `None` for a file no checkout tracks: one outside every checkout,
/// or untracked or ignored inside one, which is read as it stands. A
/// directory that does not exist holds no checkout; reading the file then
/// names it.
pub fn tracked(file: &Path) -> Result<Option<(PathBuf, String)>> {
    let directory = match file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    };
    if !directory.is_dir() {
        return Ok(None);
    }
    let toplevel = capture(
        git_command()?
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(directory),
    )?;
    if !toplevel.status.success() {
        let said = String::from_utf8_lossy(&toplevel.stderr);
        if said.contains("not a git repository") {
            return Ok(None);
        }
        bail!(
            "git could not say which checkout holds the catalog {} ({}): {}",
            file.display(),
            toplevel.status,
            said.trim()
        );
    }
    let root = PathBuf::from(String::from_utf8(toplevel.stdout)?.trim());
    let name = file
        .file_name()
        .with_context(|| format!("the catalog {} names no file", file.display()))?;
    let listed = git(
        directory,
        &["ls-files", "--full-name", "--", &name.to_string_lossy()],
    )?;
    Ok(match listed.is_empty() {
        true => None,
        false => Some((root, listed)),
    })
}

/// Say on stderr how the working tree's catalog `file` stands beside the
/// committed one a reader read: an edit that does not parse or validate is
/// named with its error and never read, and a valid edit is read once it is
/// committed. Nothing is said when the two are the same document.
pub fn report_working_tree(file: &Path, read: &Provenance, committed: &Value) {
    let at = format!("{} at {}", read.source, read.revision);
    let edit = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "the working-tree catalog {} cannot be read ({error}); {at} was read",
                file.display()
            );
            return;
        }
    };
    let parsed: Value = match serde_yaml::from_str(&edit) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!(
                "the working tree's catalog {} has an uncommitted edit that does not parse: {error}; \
                 it is not read, {at} was",
                file.display()
            );
            return;
        }
    };
    if &parsed == committed {
        return;
    }
    match super::super::validate(&parsed) {
        Ok(()) => eprintln!(
            "the working tree's catalog {} has an uncommitted edit; readers read it once it is \
             committed, and {at} was read",
            file.display()
        ),
        Err(error) => eprintln!(
            "the working tree's catalog {} has an uncommitted edit that does not validate: \
             {error:#}; it is not read, {at} was",
            file.display()
        ),
    }
}

/// What a catalog edit's answer says about who sees it: `null` for a file no
/// checkout tracks, which readers read as written; for a tracked one, that
/// readers read the committed catalog and the command that commits it.
pub fn edit_visibility(file: &Path) -> Result<Value> {
    Ok(match tracked(file)? {
        None => Value::Null,
        Some((root, relative)) => json!({
            "checkout": root,
            "path": relative,
            "readers": "read the committed catalog, so they see this edit once it is committed",
            "commit": format!("git -C {} commit -m <message> -- {relative}", root.display()),
        }),
    })
}

//! Additional local evidence for a failed `cargo fetch`: a manifest or
//! committed lock pins a commit found in a sibling checkout but absent from
//! its remote-tracking branches. Those refs are only the checkout's observed
//! state, not proof of what a remote currently holds. The fetch failure and
//! Cargo's own cause remain in the refusal.

use std::path::Path;
use std::process::{Command, Stdio};

/// Run `git` in `checkout` and return what it answered, whatever its exit:
/// the caller reads the exit as the answer it asked for. Refused only when
/// git could not be run at all.
fn git(checkout: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(checkout).args(args);
    crate::wait::output(&mut command).map_err(|error| {
        format!(
            "git {} in {} could not run: {error}",
            args.join(" "),
            checkout.display()
        )
    })
}

/// The git sources the workspace's own manifests declare, as Cargo states
/// them (`git+<url>?rev=<rev>`), read without resolving the graph; refused,
/// with Cargo's own words, when the manifests cannot be read that way.
fn manifest_sources(workspace: &Path) -> Result<Vec<String>, String> {
    let cargo = super::super::installed("cargo")
        .map_err(|error| format!("the manifests' git sources cannot be read: {error}"))?;
    let mut command = Command::new(cargo);
    command
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .args(["--manifest-path", "Cargo.toml"])
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = crate::wait::output(&mut command).map_err(|error| {
        format!(
            "cargo metadata --no-deps in {} could not run: {error}",
            workspace.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata --no-deps in {} failed with {}: {}",
            workspace.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "cargo metadata --no-deps in {} answered no JSON: {error}",
            workspace.display()
        )
    })?;
    let packages = metadata["packages"].as_array().ok_or_else(|| {
        format!(
            "cargo metadata --no-deps in {} lists no packages",
            workspace.display()
        )
    })?;
    // A dependency list in another shape is refused: read as empty, its git
    // sources went unchecked against the lock.
    let mut sources = Vec::new();
    for package in packages {
        let what = format!("the dependencies cargo metadata lists for {}", crate::cli::reporting::field(&package["name"]));
        for dependency in crate::primitives::presence::listed(&package["dependencies"], &what)? {
            if let Some(source) = dependency["source"].as_str().and_then(|source| source.strip_prefix("git+")) {
                sources.push(source.to_owned());
            }
        }
    }
    Ok(sources)
}

/// The git sources the committed `lock` pins (`git+<url>?…#<commit>`).
fn lock_sources(lock: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(lock)
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("source = \"git+")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .map(str::to_owned)
        .collect()
}

/// The repository name and commit one source names: the commit after `#`
/// when the lock resolved it, the `rev` the manifest pins otherwise.
fn repository_and_commit(source: &str) -> Option<(String, String)> {
    let (location, resolved) = match source.rsplit_once('#') {
        Some((location, commit)) => (location, Some(commit)),
        None => (source, None),
    };
    let (address, query) = match location.split_once('?') {
        Some((address, query)) => (address, Some(query)),
        None => (location, None),
    };
    let commit = match resolved {
        Some(commit) => commit.to_owned(),
        None => query?
            .split('&')
            .find_map(|pair| pair.strip_prefix("rev="))?
            .to_owned(),
    };
    let repository = address.trim_end_matches(".git").rsplit('/').next()?;
    Some((repository.to_owned(), commit))
}

/// One sentence for every git commit the manifests of `workspace` or its
/// committed `lock` pin that a sibling checkout beside `checkout` holds but
/// none of that checkout's remote-tracking branches contains; empty when no
/// sibling holds such a commit. Refused, saying why, when that cannot be
/// read: the manifests, git or the sibling's branches do not answer.
pub(super) fn unpushed(
    lock: &[u8],
    workspace: &Path,
    checkout: &Path,
) -> Result<Vec<String>, String> {
    let siblings = checkout.parent().ok_or_else(|| {
        format!(
            "{} is in no directory that could hold sibling checkouts",
            checkout.display()
        )
    })?;
    let mut pinned: Vec<(String, String)> = lock_sources(lock)
        .into_iter()
        .chain(manifest_sources(workspace)?)
        .filter_map(|source| repository_and_commit(&source))
        .collect();
    pinned.sort();
    pinned.dedup();
    let mut found = Vec::new();
    for (repository, commit) in pinned {
        let sibling = siblings.join(&repository);
        // No sibling checkout, or one that does not hold the commit, has
        // nothing local to say about it.
        let present = crate::primitives::presence::present(&sibling).map_err(|error| error.to_string())?;
        if !present || !sibling.is_dir() {
            continue;
        }
        let object = format!("{commit}^{{commit}}");
        if !git(&sibling, &["cat-file", "-e", &object])?
            .status
            .success()
        {
            continue;
        }
        let containing = git(&sibling, &["branch", "--remotes", "--contains", &commit])?;
        if !containing.status.success() {
            return Err(format!(
                "git branch --remotes --contains {commit} in {} failed with {}: {}",
                sibling.display(),
                containing.status,
                String::from_utf8_lossy(&containing.stderr).trim()
            ));
        }
        if String::from_utf8_lossy(&containing.stdout)
            .trim()
            .is_empty()
        {
            found.push(format!(
                "{repository} {commit} is in {} but no remote-tracking branch there contains \
                 it; publish the commit if this is the intended pin",
                sibling.display()
            ));
        }
    }
    Ok(found)
}

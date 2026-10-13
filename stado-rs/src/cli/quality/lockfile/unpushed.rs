//! Why `cargo fetch` could not bring a git dependency, when the reason is
//! on this machine: a manifest or the committed lock pins a commit that sits
//! in the sibling checkout of its repository and on no branch of that
//! checkout's remote. Cargo can only say the revision was not found on the
//! remote; the repository to push first is what the person pushing needs,
//! and only the canonical checkouts beside this one know it. A product that
//! pinned such a commit built nowhere but on the one machine whose cache
//! held it.

use std::path::Path;
use std::process::{Command, Stdio};

/// Run `git` in `checkout`; what it printed when it succeeded.
fn git(checkout: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(checkout).args(args);
    match crate::wait::output(&mut command) {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        _ => None,
    }
}

/// The git sources the workspace's own manifests declare, as Cargo states
/// them (`git+<url>?rev=<rev>`), read without resolving the graph. A
/// workspace whose manifests cannot be read declares none here: the lock's
/// sources are still read, and the fetch failure is reported either way.
fn manifest_sources(workspace: &Path) -> Vec<String> {
    let Ok(cargo) = super::super::installed("cargo") else {
        return Vec::new();
    };
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
    let Ok(output) = crate::wait::output(&mut command) else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Vec::new();
    };
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|package| package["dependencies"].as_array().into_iter().flatten())
        .filter_map(|dependency| dependency["source"].as_str())
        .filter_map(|source| source.strip_prefix("git+"))
        .map(str::to_owned)
        .collect()
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
/// committed `lock` pin that the sibling checkout of its repository, beside
/// `checkout`, holds while no branch of that checkout's remote contains it.
/// Empty when every pinned commit is published or no sibling checkout of its
/// repository exists.
pub(super) fn unpushed(lock: &[u8], workspace: &Path, checkout: &Path) -> Vec<String> {
    let Some(siblings) = checkout.parent() else {
        return Vec::new();
    };
    let mut pinned: Vec<(String, String)> = lock_sources(lock)
        .into_iter()
        .chain(manifest_sources(workspace))
        .filter_map(|source| repository_and_commit(&source))
        .collect();
    pinned.sort();
    pinned.dedup();
    pinned
        .into_iter()
        .filter_map(|(repository, commit)| {
            let sibling = siblings.join(&repository);
            if !sibling.is_dir() {
                return None;
            }
            git(
                &sibling,
                &["cat-file", "-e", &format!("{commit}^{{commit}}")],
            )?;
            let containing = git(&sibling, &["branch", "--remotes", "--contains", &commit])?;
            containing.trim().is_empty().then(|| {
                format!(
                    "{repository} {commit} is in {} and on no branch of its remote: push \
                     {repository} before this repository",
                    sibling.display()
                )
            })
        })
        .collect()
}

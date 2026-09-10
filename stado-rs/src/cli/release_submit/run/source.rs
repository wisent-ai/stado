//! Source identity and upload: the committed tree a submission is made of,
//! and the immutable objects it writes before anything is built.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::{Compression, GzBuilder};

use crate::cli::storage;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::PipelineChannel;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, CmdError> {
    let o = Command::new("git")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()?;
    if !o.status.success() {
        return Err(CmdError::click(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    Ok(o.stdout)
}
pub(crate) fn resolve_commit(root: &Path, requested: Option<&str>) -> Result<String, CmdError> {
    let commit = match requested {
        Some(commit) => commit.to_owned(),
        None => {
            if !git(
                root,
                &["status", "--porcelain=v1", "--untracked-files=normal"],
            )?
            .is_empty()
            {
                return Err(CmdError::click(
                    "release source must be a clean committed Git tree",
                ));
            }
            String::from_utf8(git(root, &["rev-parse", "HEAD"])?)
                .map_err(|_| CmdError::click("Git commit is not UTF-8"))?
                .trim()
                .to_string()
        }
    };
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CmdError::usage(
            "--commit must be 40 lowercase hexadecimal characters",
        ));
    }
    if git(root, &["cat-file", "-t", &commit])? != b"commit\n" {
        return Err(CmdError::usage("--commit must name a Git commit object"));
    }
    Ok(commit)
}

pub(crate) fn committed_file(root: &Path, commit: &str, path: &str) -> Result<Vec<u8>, CmdError> {
    git(root, &["show", &format!("{commit}:{path}")])
}

pub(crate) fn snapshot(root: &Path, commit: &str) -> Result<Vec<u8>, CmdError> {
    let tar = git(root, &["archive", "--format=tar", commit])?;
    let mut gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    gz.write_all(&tar)?;
    Ok(gz.finish()?)
}
pub(crate) async fn immutable(
    uri: &str,
    bytes: &[u8],
    kind: &str,
    meta: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    match storage::fetch_object_from_writer(uri).await {
        Ok(v) if v == bytes => return Ok(()),
        Ok(_) => return Err(CmdError::click(format!("immutable object differs: {uri}"))),
        Err(_) => {}
    }
    let f = tempfile::NamedTempFile::new()?;
    std::fs::write(f.path(), bytes)?;
    storage::store_object_with_metadata(uri, &f.path().display().to_string(), kind, true, meta)
        .await?;
    Ok(())
}
pub(crate) fn run_path(product: &str, id: &str, leaf: &str) -> String {
    format!("runs/release-pipeline/{product}/{id}/{leaf}")
}
pub(crate) fn run_uri(product: &str, id: &str, leaf: &str) -> String {
    format!(
        "stado://{}/{}",
        crate::config::wc_stado_storage_namespace(),
        run_path(product, id, leaf)
    )
}
pub(crate) fn run_state_path(id: &str) -> String {
    format!("runs/release-pipeline/{id}/run.json")
}
pub(crate) async fn queue_immutable(path: &str, bytes: &[u8]) -> Result<(), CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(existing) = store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(CmdError::click(format!(
                "immutable queue object differs: {path}"
            )))
        };
    }
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), bytes)?;
    if store
        .upload_file_if_absent(path, file.path())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(());
    }
    match store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        Some(existing) if existing == bytes => Ok(()),
        _ => Err(CmdError::click(format!(
            "immutable queue object raced with different bytes: {path}"
        ))),
    }
}
pub(crate) fn identity(
    product: &str,
    version: &str,
    channel: PipelineChannel,
    source: &str,
    manifest: &str,
) -> String {
    release_control::sha256_bytes(
        format!("{product}\0{version}\0{channel:?}\0{source}\0{manifest}").as_bytes(),
    )[..32]
        .into()
}

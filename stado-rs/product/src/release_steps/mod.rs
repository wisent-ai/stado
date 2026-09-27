//! Release-manifest steps that products carried as copied scripts.
//!
//! `scripts/stado_release.py` was copied into codespy, growth-tactics,
//! OpenEnv, versioning, wisent-extractors, wisent-evaluators,
//! wisent-optimizer and wisent-visuals. The scripts directories were removed
//! on the operator's instruction (no scripts in our repositories), and every
//! build of those products has failed at its first step since. What the copies
//! did is two capabilities, now one command each:
//!
//! - `stado product source-bundle`: the checkout's files in a reproducible
//!   `release/source-bundle.tar`, with each file's digest in
//!   `output/build-metadata.json` inside it.
//! - `stado product python build` / `deliver-pypi` (module `python`): a Python
//!   package's wheel and sdist in `release/python-distributions.tar`, and
//!   their upload to PyPI from the verified release archive.

mod python;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

pub use python::run as run_python;

/// Every archive entry's modification time: 2000-01-01T00:00:00Z, the value
/// the replaced scripts used, so a bundle rebuilt from one commit keeps the
/// digest earlier releases of these products were published under.
const ARCHIVE_EPOCH: u64 = 946_684_800;

/// Directories no release carries: version control and interpreter caches.
const EXCLUDED: [&str; 5] = [
    ".git",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];

/// One variable of the release worker's contract, refused by name when absent.
pub(crate) fn required(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{name} is required and was not provided"))
}

pub(crate) fn output_dir() -> Result<PathBuf> {
    let output = PathBuf::from(required("WISENT_OUTPUT_DIR")?);
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    fs::create_dir_all(&output)?;
    Ok(output)
}

/// A reproducible tar: no owner, one fixed time, mode reduced to 0644 or 0755.
pub(crate) fn archive_entry(
    archive: &mut tar::Builder<fs::File>,
    name: &str,
    bytes: &[u8],
    executable: bool,
) -> Result<()> {
    let mut header = tar::Header::new_ustar();
    header.set_size(bytes.len() as u64);
    header.set_mode(if executable { 0o755 } else { 0o644 });
    header.set_uid(u64::MIN);
    header.set_gid(u64::MIN);
    header.set_mtime(ARCHIVE_EPOCH);
    header.set_entry_type(tar::EntryType::Regular);
    archive
        .append_data(&mut header, name, bytes)
        .with_context(|| format!("adding {name} to the release archive"))
}

fn source_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| EXCLUDED.contains(&name))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!(
                "{} is a symbolic link; a source bundle carries files, not links",
                path.strip_prefix(root).unwrap_or(&path).display()
            );
        }
        if metadata.is_dir() {
            source_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

/// `stado product source-bundle`: run by a release manifest's build step.
pub fn run_source_bundle() -> Result<i32> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    if !source.is_dir() {
        bail!("WISENT_SOURCE_DIR is not a directory: {}", source.display());
    }
    let product = required("WISENT_PRODUCT")?;
    let version = required("WISENT_VERSION")?;
    let mut files = Vec::new();
    source_files(&source, &source, &mut files)?;
    if files.is_empty() {
        bail!("the source checkout {} holds no files", source.display());
    }
    files.sort_by(|left, right| {
        left.strip_prefix(&source)
            .unwrap()
            .cmp(right.strip_prefix(&source).unwrap())
    });

    let release = output_dir()?.join("release");
    if release.exists() {
        fs::remove_dir_all(&release)?;
    }
    fs::create_dir_all(&release)?;
    let bundle = release.join("source-bundle.tar");
    let mut archive = tar::Builder::new(fs::File::create(&bundle)?);
    let mut digests = serde_json::Map::new();
    for path in &files {
        let relative = path
            .strip_prefix(&source)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let mut bytes = Vec::new();
        fs::File::open(path)?.read_to_end(&mut bytes)?;
        digests.insert(relative.clone(), json!(hex::encode(Sha256::digest(&bytes))));
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path)?.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        archive_entry(
            &mut archive,
            &format!("source/{relative}"),
            &bytes,
            executable,
        )?;
    }
    let metadata = json!({"product": product, "version": version, "files": digests});
    archive_entry(
        &mut archive,
        "output/build-metadata.json",
        format!("{metadata}\n").as_bytes(),
        false,
    )?;
    archive.finish()?;
    println!("staged {} ({} files)", bundle.display(), files.len());
    Ok(0)
}

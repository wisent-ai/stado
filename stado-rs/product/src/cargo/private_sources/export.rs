use super::{locked_packages, source_config, Provenance, PROVENANCE_SCHEMA_VERSION};
use crate::common::{checked, sha256, toolchain_command};
use anyhow::{bail, Context, Result};
use flate2::{Compression, GzBuilder};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The publisher retains this owner until the immutable object is stored.
pub struct Export {
    pub archive: PathBuf,
    pub sha256: String,
    directory: PathBuf,
}

impl Drop for Export {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub fn export(root: &Path) -> Result<Export> {
    let root = fs::canonicalize(root)?;
    let packages = locked_packages(&root)?;
    if packages.is_empty() {
        bail!(
            "{} locks no private Git packages",
            root.join("Cargo.lock").display()
        );
    }
    let lock_digest = sha256(&root.join("Cargo.lock"))?;
    let directory = root
        .join(".build/private-cargo-sources")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory)?;
    let mut exported = Export {
        archive: directory.join("source.tar.gz"),
        sha256: String::new(),
        directory,
    };
    let vendor = exported.directory.join("vendor");
    checked(
        toolchain_command("cargo")
            .args(["vendor", "--locked", "--versioned-dirs", "--quiet"])
            .arg(&vendor)
            .current_dir(&root),
    )?;
    let payload = exported.directory.join("payload");
    let sources = payload.join("sources");
    fs::create_dir_all(&sources)?;
    for package in &packages {
        let name = format!("{}-{}", package.name, package.version);
        let source = vendor.join(&name);
        if !source.join(".cargo-checksum.json").is_file() {
            bail!(
                "Cargo did not vendor checksum-protected source for {} from {}",
                name,
                package.source
            );
        }
        fs::rename(&source, sources.join(&name))
            .with_context(|| format!("staging private Cargo source {}", source.display()))?;
    }
    if sha256(&root.join("Cargo.lock"))? != lock_digest {
        bail!("Cargo.lock changed while exporting private sources; nothing was published");
    }
    fs::write(payload.join("config.toml"), source_config(&packages)?)?;
    let provenance = Provenance {
        schema_version: PROVENANCE_SCHEMA_VERSION,
        cargo_lock_sha256: lock_digest,
        packages,
    };
    fs::write(
        payload.join("provenance.json"),
        serde_json::to_vec_pretty(&provenance)?,
    )?;
    let file = File::create(&exported.archive)?;
    let encoder = GzBuilder::new().write(file, Compression::best());
    let mut archive = tar::Builder::new(encoder);
    archive.mode(tar::HeaderMode::Deterministic);
    archive.follow_symlinks(false);
    append(&mut archive, &payload, Path::new(""))?;
    archive.into_inner()?.finish()?.sync_all()?;
    exported.sha256 = sha256(&exported.archive)?;
    Ok(exported)
}

fn append<W: Write>(archive: &mut tar::Builder<W>, root: &Path, relative: &Path) -> Result<()> {
    let mut entries = fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            archive.append_dir(&path, entry.path())?;
            append(archive, root, &path)?;
        } else if kind.is_file() {
            archive.append_file(&path, &mut File::open(entry.path())?)?;
        } else {
            bail!(
                "private Cargo source contains a symlink or special file: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

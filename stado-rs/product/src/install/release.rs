use super::plan::{Placement, Prepared};
use crate::{
    catalog::text,
    common::{atomic_json, checked, file_members, platform, sha256, unpack, Runtime},
    paths, signing,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn prepare(
    product: &Value,
    version: &str,
    revision: &str,
    runtime: &Runtime,
) -> Result<Prepared> {
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        bail!("--source-commit requires a full lowercase Git commit");
    }
    if version.is_empty()
        || !version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        bail!("--release-version must be a canonical version coordinate");
    }
    let id = text(product, "id")?;
    let platform = platform()?;
    let output = runtime
        .output
        .join("release-install")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let archive = output.join(format!("{id}-{version}-{platform}.tar.gz"));
    let fetched = checked(
        crate::common::stado()
            .args([
                "release",
                "fetch",
                id,
                version,
                "--platform",
                &platform,
                "--source-commit",
                revision,
                "--destination",
            ])
            .arg(&archive)
            .arg("--json"),
    )?;
    let mut receipt: Value = serde_json::from_slice(&fetched.stdout)
        .context("reading Stado's signed release receipt")?;
    let coordinate = &receipt["coordinate"];
    if coordinate["product"] != id
        || coordinate["version"] != version
        || coordinate["platform"] != platform
        || coordinate["source_revision"] != revision
        || receipt["artifact"]["source_revision"] != revision
    {
        bail!("Stado release receipt attests a different coordinate; nothing was installed");
    }
    if receipt["destination"].as_str() != archive.to_str() {
        bail!("Stado release receipt names another archive destination");
    }
    let artifact = &receipt["artifact"];
    if artifact["key_id"].as_str().is_none_or(str::is_empty)
        || artifact["manifest_sha256"]
            .as_str()
            .is_none_or(|s| s.len() != 64)
    {
        bail!("Stado release receipt has no signing-key or manifest attestation");
    }
    let expected_hash = text(artifact, "artifact_sha256")?;
    if sha256(&archive)? != expected_hash {
        bail!("fetched archive differs from its signed digest; nothing was installed");
    }
    atomic_json(&output.join("fetch.json"), &receipt)?;
    let extracted = output.join("archive");
    unpack(&archive, &extracted)?;
    let mut placements = Vec::new();
    let bin = extracted.join("bin");
    if bin.is_dir() {
        for entry in fs::read_dir(&bin)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                bail!(
                    "release bin member must be a regular executable: {}",
                    entry.path().display()
                );
            }
            let destination = runtime.home.join(".stado/bin").join(entry.file_name());
            add_binary(&entry.path(), &destination, &mut placements, runtime)?;
        }
    } else {
        let source = extracted.join(id);
        if source.is_file() {
            add_binary(
                &source,
                &runtime.home.join(".stado/bin").join(id),
                &mut placements,
                runtime,
            )?;
        }
    }
    if placements.is_empty() {
        bail!("signed release contains no executable bin members or root product executable");
    }
    let resources = extracted.join("share").join(id);
    if resources.is_dir() {
        placements.push(Placement {
            source: resources,
            destination: runtime.home.join(".stado/share").join(id),
            symbolic: false,
        });
    }
    let mut files = Vec::new();
    for placement in &placements {
        if placement.symbolic {
            files.push(json!({"path": placement.destination, "link": placement.source}));
            continue;
        }
        let members = if placement.source.is_dir() {
            file_members(&placement.source)?
        } else {
            vec![placement.source.clone()]
        };
        for source in members {
            let destination = if placement.source.is_dir() {
                placement
                    .destination
                    .join(source.strip_prefix(&placement.source)?)
            } else {
                placement.destination.clone()
            };
            if source.symlink_metadata()?.file_type().is_symlink() {
                files.push(json!({"path": destination, "link": fs::read_link(&source)?}));
                continue;
            }
            if signing::native(&source)? {
                let identity = signing::inspect(&source)?;
                if !signing::acceptable(&identity) {
                    bail!(
                        "signed archive contains unstable native code {}: {}",
                        source.display(),
                        identity["error"]
                    );
                }
                signing::verify_previous(&source, &destination)?;
            }
            let mut file = json!({"path": destination, "sha256": sha256(&source)?});
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file["mode"] = json!(source.metadata()?.permissions().mode() & 0o777);
            }
            files.push(file);
        }
    }
    receipt["files"] = json!(files);
    receipt["evidence"] = json!(output);
    Ok(Prepared {
        placements,
        source_revision: revision.to_owned(),
        source_directory: None,
        release: Some(receipt),
    })
}

fn add_binary(
    source: &Path,
    destination: &Path,
    placements: &mut Vec<Placement>,
    runtime: &Runtime,
) -> Result<()> {
    if !paths::executable(source) {
        bail!("release binary is not executable: {}", source.display());
    }
    placements.push(Placement {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        symbolic: false,
    });
    placements.push(Placement {
        source: destination.to_path_buf(),
        destination: runtime.home.join(".local/bin").join(
            destination
                .file_name()
                .context("release binary has no filename")?,
        ),
        symbolic: true,
    });
    Ok(())
}

pub fn verify_files(receipt: &Value) -> Result<()> {
    let files = receipt["files"]
        .as_array()
        .context("release receipt has no installed-file evidence")?;
    if files.is_empty() {
        bail!("release receipt has no installed-file evidence");
    }
    for file in files {
        let path = PathBuf::from(text(file, "path")?);
        if let Some(target) = file["link"].as_str() {
            if fs::read_link(&path)? != Path::new(target) {
                bail!("release link changed: {}", path.display());
            }
        } else {
            if path.symlink_metadata()?.file_type().is_symlink() {
                bail!("release file became a symlink: {}", path.display());
            }
            if sha256(&path)? != text(file, "sha256")? {
                bail!("installed release content differs: {}", path.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(expected) = file["mode"].as_u64() {
                    if u64::from(path.metadata()?.permissions().mode() & 0o777) != expected {
                        bail!("release file mode changed: {}", path.display());
                    }
                }
            }
        }
    }
    Ok(())
}

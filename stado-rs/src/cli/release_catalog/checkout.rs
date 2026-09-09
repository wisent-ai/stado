use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{self, PRODUCT_MANIFEST};

use super::{product, publish_entry};

fn scan(root: &Path, found: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    // A release declaration belongs to the checkout that contains it. Never
    // descend into that checkout's build products or dependency clones: a
    // copied dependency manifest is not another product checked out by the
    // operator.
    let manifest = root.join(PRODUCT_MANIFEST);
    if manifest.is_file() {
        found.push(manifest);
        return Ok(());
    }
    if root.join(".git").exists() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(root)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".build" | ".git" | "target" | "node_modules" | ".venv" | "dist" | "build"
        ) {
            continue;
        }
        scan(&entry.path(), found)?;
    }
    Ok(())
}

pub(super) async fn sync(root: &Path, json: bool) -> Result<(), CmdError> {
    let root = root.canonicalize()?;
    let mut paths = Vec::new();
    scan(&root, &mut paths)?;
    let mut declarations = BTreeMap::new();
    for path in paths {
        let bytes = std::fs::read(&path)?;
        let manifest = release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?;
        let name = product(&manifest).to_string();
        if declarations
            .insert(name.clone(), (manifest, bytes))
            .is_some()
        {
            return Err(CmdError::click(format!(
                "catalog sync found duplicate product {name:?}"
            )));
        }
    }
    if declarations.is_empty() {
        return Err(CmdError::click(format!(
            "{} contains no {PRODUCT_MANIFEST}",
            root.display()
        )));
    }
    let mut entries = Vec::new();
    for (_, (manifest, bytes)) in declarations {
        entries.push(publish_entry(manifest, release_control::sha256_bytes(&bytes), None).await?);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in entries {
            println!(
                "cataloged {} manifest={}",
                entry.product, entry.manifest_sha256
            );
        }
    }
    Ok(())
}

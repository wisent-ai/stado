pub mod build;
mod packages;
use crate::{
    catalog::text,
    common::{checked, Runtime},
    install::plan::{Placement, Prepared},
    source,
};
use anyhow::{bail, Result};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

pub fn prepare(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    surface: &str,
    root: &Path,
) -> Result<Prepared> {
    if surface == "desktop" {
        return build::desktop(runtime, product, recipe, root);
    }
    match text(recipe, "kind")? {
        "stado-release" => build::release(runtime, product, recipe, root),
        "cargo" | "npm" | "pip" | "pipx" => packages::prepare(runtime, product, recipe, root),
        "local-build" => {
            let binaries = packages::names(recipe, "binaries")?;
            let home = root
                .join(".build/local-install")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(home.join(".local/bin"))?;
            let evidence = root
                .join(".wisent-output/local-install")
                .join(uuid::Uuid::new_v4().to_string());
            let recorded = source::snapshot(root, &evidence, &home)?;
            checked(
                Command::new("/bin/sh")
                    .args(["-c", text(recipe, "command")?])
                    .current_dir(root)
                    .env("HOME", &home)
                    .env("WISENT_WORKSPACE", &runtime.workspace)
                    .env("WISENT_OUTPUT_DIR", &evidence),
            )?;
            source::verify_unchanged(root, &recorded, &evidence, &home)?;
            let mut placements = Vec::new();
            for binary in binaries {
                let source = home.join(".local/bin").join(&binary);
                if !source.is_file() || source.symlink_metadata()?.file_type().is_symlink() {
                    bail!("local build did not produce regular binary {} in its isolated installation home", source.display());
                }
                placements.push(Placement {
                    source,
                    destination: runtime.home.join(".local/bin").join(binary),
                    symbolic: false,
                });
            }
            Ok(Prepared {
                placements,
                source_revision: recorded["revision"].as_str().unwrap().to_owned(),
                source_directory: Some(root.to_path_buf()),
                release: None,
            })
        }
        kind => bail!("unsupported {surface} installation recipe {kind}"),
    }
}

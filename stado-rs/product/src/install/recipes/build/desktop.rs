//! A desktop product built from source in its workspace and placed as the
//! bundle the build produced. The workspace's `.build` is reused by the next
//! build, so the installation tags it as a cache instead of removing it.

use super::{evidence, manifest};
use crate::{
    catalog::text,
    common::{checked, Runtime},
    install::plan::{Placement, Prepared},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

pub fn desktop(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
) -> Result<Prepared> {
    if !cfg!(target_os = "macos") {
        bail!("desktop bundle installation requires macOS");
    }
    let run = evidence::directory(root)?;
    let evidence = run.path.clone();
    let recorded = source::snapshot(root, &evidence, &root.join(".build/wisent-source"))?;
    let document = if recipe["kind"] == "desktop-release" {
        manifest::load(root, text(recipe, "manifest")?)?
    } else {
        recipe.clone()
    };
    let command = text(
        &document,
        if recipe["kind"] == "desktop-release" {
            "build_command"
        } else {
            "command"
        },
    )?;
    checked(
        Command::new("/bin/sh")
            .args(["-c", command])
            .current_dir(root)
            .envs(manifest::secrets(&[&document])?),
    )?;
    source::verify_unchanged(
        root,
        &recorded,
        &evidence,
        &root.join(".build/wisent-source"),
    )?;
    let source = if let Some(path) = document["bundle_path"].as_str() {
        manifest::inside(root, path)?
    } else {
        let directory = root.join(".build");
        let expected = directory.join(format!("{}.app", text(product, "name")?.replace(' ', "")));
        if expected.is_dir() {
            expected
        } else {
            let mut bundles = Vec::new();
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.is_dir() && path.extension().is_some_and(|s| s == "app") {
                    bundles.push(path);
                }
            }
            let count = bundles.len();
            let mut found = bundles.into_iter();
            match (found.next(), found.next()) {
                (Some(only), None) => only,
                _ => bail!(
                    "desktop build produced {count} candidate bundles in {}",
                    directory.display()
                ),
            }
        }
    };
    if !source.is_dir() {
        bail!(
            "desktop build did not produce a bundle: {}",
            source.display()
        );
    }
    let destination = runtime
        .home
        .join("Applications")
        .join(source.file_name().context("bundle has no filename")?);
    Ok(Prepared {
        placements: vec![Placement {
            source,
            destination,
            symbolic: false,
        }],
        source_revision: recorded["revision"]
            .as_str()
            .context("snapshot has no revision")?
            .to_owned(),
        source_directory: Some(root.to_path_buf()),
        release: None,
        scratch: None,
        cache: Some(root.join(".build")),
    })
}

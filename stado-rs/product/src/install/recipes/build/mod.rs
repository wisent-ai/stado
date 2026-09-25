mod inputs;
pub mod manifest;
mod mounts;
use crate::{
    catalog::text,
    common::{atomic_json, checked, platform, relative, Runtime},
    install::plan::{Placement, Prepared},
    signing, source,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn step(step: &Value, root: &Path, environment: &BTreeMap<String, String>) -> Result<()> {
    let argv: Vec<&str> = step["argv"]
        .as_array()
        .context("release command requires argv")?
        .iter()
        .map(|value| value.as_str().context("release argv must contain strings"))
        .collect::<Result<_>>()?;
    let (program, arguments) = argv.split_first().context("release argv is empty")?;
    checked(
        Command::new(program)
            .args(arguments)
            .envs(environment)
            .current_dir(root),
    )?;
    Ok(())
}

pub fn release(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
) -> Result<Prepared> {
    let id = text(product, "id")?;
    let document = manifest::load(root, text(recipe, "manifest")?)?;
    if document["product"] != id {
        bail!("release manifest product does not match {id}");
    }
    let platform = platform()?;
    let spec = document["platforms"]
        .get(&platform)
        .with_context(|| format!("{id} has no {platform} release"))?;
    let evidence = root
        .join(".wisent-output/install")
        .join(uuid::Uuid::new_v4().to_string());
    let output = evidence.join("output");
    let inputs = evidence.join("inputs");
    fs::create_dir_all(&output)?;
    let recorded = source::snapshot(root, &evidence, &root.join(".build/wisent-source"))?;
    let revision = recorded["revision"]
        .as_str()
        .context("source snapshot has no revision")?
        .to_owned();
    let mut environment = BTreeMap::from([
        (
            "WISENT_SOURCE_DIR".to_owned(),
            root.to_string_lossy().into_owned(),
        ),
        (
            "WISENT_SOURCE_COMMIT".to_owned(),
            revision.trim_end_matches("-dirty").to_owned(),
        ),
        (
            "WISENT_OUTPUT_DIR".to_owned(),
            output.to_string_lossy().into_owned(),
        ),
        (
            "WISENT_INPUTS_DIR".to_owned(),
            inputs.to_string_lossy().into_owned(),
        ),
        ("WISENT_PRODUCT".to_owned(), id.to_owned()),
        (
            "WISENT_VERSION".to_owned(),
            manifest::version(root, &document)?,
        ),
        ("WISENT_PLATFORM".to_owned(), platform.clone()),
    ]);
    environment.extend(inputs::materialise(runtime, &document, spec, &inputs)?);
    environment.extend(manifest::secrets(&[&document, spec])?);
    if let Some(quality) = spec.get("quality") {
        for check in quality
            .as_array()
            .context("release quality must be an array")?
        {
            step(check, root, &environment).with_context(|| {
                format!(
                    "{id} quality {} failed; build evidence: {}",
                    check["name"],
                    evidence.display()
                )
            })?;
        }
    }
    step(&spec["build"], root, &environment)
        .with_context(|| format!("{id} build failed; evidence: {}", evidence.display()))?;
    source::verify_unchanged(
        root,
        &recorded,
        &evidence,
        &root.join(".build/wisent-source"),
    )?;
    if platform.starts_with("darwin-") {
        signing::stage(
            &manifest::inside(root, text(recipe, "manifest")?)?,
            &output,
            &platform,
        )?;
    }
    let runtime_binary = document["runtime"]["binary"].as_str().unwrap_or(id);
    let mut placements = Vec::new();
    let mut binaries = 0;
    for (source_name, member) in spec["stage"]
        .as_object()
        .context("release platform has no stage map")?
    {
        let member = member.as_str().context("stage member must be a path")?;
        let member_path = relative(Path::new(member))?;
        let root_binary = member == runtime_binary && member_path.components().count() == 1;
        let binary = member.starts_with("bin/") || root_binary;
        if !binary && !member.starts_with(&format!("share/{id}/")) {
            continue;
        }
        let source = manifest::inside(&output, source_name).with_context(|| {
            format!(
                "{id} stage key '{source_name}' is relative to output {}; build evidence: {}",
                output.display(),
                evidence.display()
            )
        })?;
        let destination = runtime.home.join(".stado").join(if root_binary {
            format!("bin/{member}")
        } else {
            member.to_owned()
        });
        if binary {
            if !source.is_file() || member_path.components().count() > 2 {
                bail!("CLI stage member must be one regular binary under bin/: {member}");
            }
            placements.push(Placement {
                source: destination.clone(),
                destination: runtime
                    .home
                    .join(".local/bin")
                    .join(destination.file_name().unwrap()),
                symbolic: true,
            });
            binaries += 1;
        }
        placements.push(Placement {
            source,
            destination,
            symbolic: false,
        });
    }
    if binaries == 0 {
        bail!("{id} stages no CLI binary into bin/; nothing was installed");
    }
    placements.sort_by_key(|placement| placement.symbolic);
    atomic_json(
        &evidence.join("prepared.json"),
        &json!({"product": id, "source_revision": revision, "placements": placements}),
    )?;
    Ok(Prepared {
        placements,
        source_revision: revision,
        source_directory: Some(root.to_path_buf()),
        release: None,
    })
}

pub fn desktop(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
) -> Result<Prepared> {
    if !cfg!(target_os = "macos") {
        bail!("desktop bundle installation requires macOS");
    }
    let evidence = root
        .join(".wisent-output/install")
        .join(uuid::Uuid::new_v4().to_string());
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
            if bundles.len() != 1 {
                bail!(
                    "desktop build produced {} candidate bundles in {}",
                    bundles.len(),
                    directory.display()
                );
            }
            bundles.remove(0)
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
    })
}

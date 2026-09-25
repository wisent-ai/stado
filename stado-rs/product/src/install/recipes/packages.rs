use crate::{
    cargo,
    catalog::text,
    common::{checked, relative, Runtime},
    install::plan::{Placement, Prepared},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

pub fn names(recipe: &Value, key: &str) -> Result<Vec<String>> {
    let names: Vec<_> = recipe[key]
        .as_array()
        .with_context(|| format!("recipe requires {key}"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("binary names must be strings")
        })
        .collect::<Result<_>>()?;
    if names.is_empty() {
        bail!("recipe {key} is empty");
    }
    for name in &names {
        if relative(Path::new(name))?.components().count() != 1 {
            bail!("binary name must be one filename: {name}");
        }
    }
    Ok(names)
}

pub fn prepare(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
) -> Result<Prepared> {
    let kind = text(recipe, "kind")?;
    let id = text(product, "id")?;
    let binaries = names(
        recipe,
        if kind == "pip" {
            "entrypoints"
        } else {
            "binaries"
        },
    )?;
    if kind == "cargo" {
        let staging = root
            .join(".wisent-output/cargo-install")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&staging)?;
        let mut arguments = vec![
            "--release".to_owned(),
            "--target-dir".to_owned(),
            staging.to_string_lossy().into_owned(),
        ];
        for binary in &binaries {
            arguments.extend(["--bin".to_owned(), binary.clone()]);
        }
        if let Some(features) = recipe.get("features") {
            let features = features
                .as_array()
                .context("cargo features must be an array")?
                .iter()
                .map(|value| value.as_str().context("cargo features must be strings"))
                .collect::<Result<Vec<_>>>()?;
            if !features.is_empty() {
                arguments.extend(["--features".to_owned(), features.join(",")]);
            }
        }
        let built = cargo::execute(
            runtime,
            &super::build::manifest::inside(root, text(recipe, "manifest")?)?,
            "build",
            &arguments,
        )?;
        if built.code != 0 {
            bail!(
                "Cargo build of {} failed: {}; evidence {}",
                text(recipe, "manifest")?,
                built.report["error"]
                    .as_str()
                    .unwrap_or("no error was recorded"),
                built.report["evidence"].as_str().unwrap_or("unrecorded")
            );
        }
        let mut placements = Vec::new();
        for binary in binaries {
            let source = staging.join("release").join(&binary);
            if !source.is_file() {
                bail!(
                    "Cargo reported success but did not produce {}",
                    source.display()
                );
            }
            placements.push(Placement {
                source,
                destination: runtime.home.join(".local/bin").join(binary),
                symbolic: false,
            });
        }
        return Ok(Prepared {
            placements,
            source_revision: source::revision(root)?,
            source_directory: Some(root.to_path_buf()),
            release: None,
        });
    }
    // Versioned package roots keep generated absolute interpreter paths valid. Only the exposed links change at activation.
    let parent = runtime.home.join(".stado/packages").join(id);
    fs::create_dir_all(&parent)?;
    let prefix = parent.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&prefix)?;
    let scratch = root
        .join(".build/package-install")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&scratch)?;
    let mut command = match kind {
        "npm" => {
            let mut command = Command::new("npm");
            command
                .args(["install", "--global", "--install-links", "--prefix"])
                .arg(&prefix)
                .arg(root);
            command
        }
        "pip" | "pipx" => {
            let mut command = Command::new("pipx");
            command.args(["install", "--force"]);
            if kind == "pip" {
                command.arg(text(recipe, "package")?);
            } else {
                command.arg(root);
            }
            command
                .env("PIPX_HOME", prefix.join("pipx"))
                .env("PIPX_BIN_DIR", prefix.join("bin"))
                .env("PIPX_MAN_DIR", prefix.join("man"));
            command
        }
        _ => bail!("unsupported package recipe {kind}"),
    };
    let installation = checked(command.env("TMPDIR", &scratch).current_dir(root));
    fs::remove_dir_all(&scratch)?;
    installation?;
    let mut placements = vec![Placement {
        source: prefix.clone(),
        destination: prefix.clone(),
        symbolic: false,
    }];
    for binary in binaries {
        let executable = prefix.join("bin").join(&binary);
        if !executable.is_file() {
            bail!(
                "package installer reported success but {} is absent",
                executable.display()
            );
        }
        placements.push(Placement {
            source: executable,
            destination: runtime.home.join(".local/bin").join(binary),
            symbolic: true,
        });
    }
    Ok(Prepared {
        placements,
        source_revision: if kind == "pip" {
            format!("package:{}", text(recipe, "package")?)
        } else {
            source::revision(root)?
        },
        source_directory: if kind == "pip" {
            None
        } else {
            Some(root.to_path_buf())
        },
        release: None,
    })
}

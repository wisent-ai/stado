use crate::support::{command, Run};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{env, path::PathBuf, process::Command};

pub struct Coordinate {
    pub version: String,
    pub source: String,
}

pub struct Releases {
    pub older: Coordinate,
    pub newer: Coordinate,
    pub config: PathBuf,
}

pub fn published(run: &mut Run) -> Result<Releases> {
    let mut inspect = Command::new(&run.binary);
    inspect.args(["config", "show"]);
    let config = command(run, inspect)?.json()?;
    let config = PathBuf::from(
        config["file"]
            .as_str()
            .context("real Stado configuration file is required for isolated consumers")?,
    )
    .canonicalize()?;
    let mut status = Command::new(&run.binary);
    status
        .args(["release", "status", "jeden", "--json"])
        .env("STADO_CONFIG", &config);
    let response = command(run, status)?.json()?;
    let platform = stado_product::common::platform()?;
    let mut published: Vec<_> = response["runs"]
        .as_array()
        .context("actual release runs")?
        .iter()
        .filter(|run| {
            run["state"] == "completed" && run["platforms"][&platform]["state"] == "published"
        })
        .collect();
    published.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    let mut coordinates = Vec::<Coordinate>::new();
    for release in published {
        let source = release["source_commit"]
            .as_str()
            .context("published source commit")?;
        if coordinates
            .iter()
            .any(|coordinate| coordinate.source == source)
        {
            continue;
        }
        coordinates.push(Coordinate {
            version: release["version"]
                .as_str()
                .context("published version")?
                .to_owned(),
            source: source.to_owned(),
        });
        if coordinates.len() == 2 {
            break;
        }
    }
    ensure!(coordinates.len() == 2, "two real published Jeden releases are required on {platform}; no lifecycle pass was performed");
    let older = coordinates.pop().context("older published coordinate")?;
    let newer = coordinates.pop().context("newer published coordinate")?;
    Ok(Releases {
        older,
        newer,
        config,
    })
}

pub fn product(run: &Run, releases: &Releases, arguments: &[&str]) -> Result<Command> {
    let mut cmd = run.product(arguments);
    let inherited = env::var_os("PATH").context("real dependency PATH is required")?;
    let path = env::join_paths(
        [run.home.join(".local/bin"), run.home.join(".stado/bin")]
            .into_iter()
            .chain(env::split_paths(&inherited)),
    )?;
    cmd.env("STADO_CONFIG", &releases.config).env("PATH", path);
    Ok(cmd)
}

pub fn install(
    run: &mut Run,
    releases: &Releases,
    action: &str,
    coordinate: &Coordinate,
) -> Result<Value> {
    let cmd = product(
        run,
        releases,
        &[
            action,
            "jeden",
            "--surface",
            "cli",
            "--release-version",
            &coordinate.version,
            "--source-commit",
            &coordinate.source,
            "--json",
        ],
    )?;
    command(run, cmd)?.json()
}

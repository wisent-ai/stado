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
    let newest = response["runs"]
        .as_array()
        .context("actual release runs")?
        .iter()
        .filter(|run| run["platforms"][&platform]["state"] == "published")
        .max_by(|a, b| a["created_at"].as_str().cmp(&b["created_at"].as_str()))
        .and_then(|run| run["version"].as_str())
        .with_context(|| format!("no Jeden run has published {platform}"))?
        .to_owned();
    // The run record is not the authority on what can be installed; the
    // release route is. Runs of jeden 0.1.16 and 0.1.17 read `published`
    // while neither `release.json` was there, and `release status` lists
    // only recent runs, so the journey asks the route itself: every patch
    // version from the newest published one down, each manifest read
    // through the same public route an install uses. A builder holds no
    // publisher credential to list the store instead.
    let (line, patch) = newest
        .rsplit_once('.')
        .with_context(|| format!("{newest} is not a MAJOR.MINOR.PATCH version"))?;
    let patch: u64 = patch
        .parse()
        .with_context(|| format!("{newest} has no numeric patch"))?;
    let mut coordinates = Vec::<Coordinate>::new();
    for candidate in (0..=patch).rev() {
        let version = format!("{line}.{candidate}");
        let uri = format!("stado://releases/jeden/{version}/{platform}/release.json");
        let mut stat = Command::new(&run.binary);
        stat.args(["storage", "stat", &uri, "--json"])
            .env("STADO_CONFIG", &config);
        if command(run, stat)?.json()?["state"] != "present" {
            continue;
        }
        let mut read = Command::new(&run.binary);
        read.args(["storage", "cat", &uri])
            .env("STADO_CONFIG", &config);
        let manifest = command(run, read)?.json()?;
        let source = manifest["source_revision"]
            .as_str()
            .context("published source revision")?;
        if coordinates
            .iter()
            .any(|coordinate| coordinate.source == source)
        {
            continue;
        }
        coordinates.push(Coordinate {
            version,
            source: source.to_owned(),
        });
        if coordinates.len() == 2 {
            break;
        }
    }
    ensure!(coordinates.len() == 2, "two real published Jeden releases are required in the release store on {platform}; no lifecycle pass was performed");
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

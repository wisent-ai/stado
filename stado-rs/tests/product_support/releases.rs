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
    // The release store is the authority on what can be installed. Run
    // records are not: on 2026-09-25 runs of jeden 0.1.16 and 0.1.17 read
    // `completed`/`published` while neither `release.json` existed, so a
    // journey chosen from them failed on a fetch no install could satisfy.
    let platform = stado_product::common::platform()?;
    let mut listing = Command::new(&run.binary);
    listing
        .args(["storage", "objects", "releases", "jeden/", "--json"])
        .env("STADO_CONFIG", &config);
    let objects = command(run, listing)?.json()?;
    let suffix = format!("/{platform}/release.json");
    let mut manifests: Vec<(String, String)> = objects["objects"]
        .as_array()
        .context("actual release store objects")?
        .iter()
        .filter_map(|object| {
            let uri = object["uri"].as_str()?;
            uri.ends_with(&suffix).then(|| {
                (
                    object["updated_at"].as_str().unwrap_or_default().to_owned(),
                    uri.to_owned(),
                )
            })
        })
        .collect();
    manifests.sort_by(|a, b| b.0.cmp(&a.0));
    let mut coordinates = Vec::<Coordinate>::new();
    for (_, uri) in manifests {
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
            version: manifest["version"]
                .as_str()
                .context("published version")?
                .to_owned(),
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

#[path = "../product_support/mod.rs"]
pub mod support;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use stado_product::common::sha256;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::releases::{self, install, product, Coordinate, Releases};
use support::{command, Run};

#[test]
fn signed_installation_updates_rolls_back_and_refuses_another_source() -> Result<()> {
    let mut run = Run::new("releases")?;
    let result = journey(&mut run);
    run.finish(result)
}

fn state_path(run: &Run) -> PathBuf {
    run.home.join(".stado/products/jeden/cli.json")
}

fn receipt(run: &Run, label: &str) -> Result<Value> {
    let bytes = fs::read(state_path(run))?;
    fs::write(run.evidence.join(format!("{label}.json")), &bytes)?;
    serde_json::from_slice(&bytes).context("persisted lifecycle receipt")
}

fn ready(run: &mut Run, releases: &Releases, coordinate: &Coordinate) -> Result<()> {
    let cmd = product(
        run,
        releases,
        &["status", "jeden", "--surface", "cli", "--json"],
    )?;
    let status = command(run, cmd)?.json()?;
    ensure!(
        status["readiness"]["ready"] == true,
        "actual installed readiness failed: {status}"
    );
    ensure!(
        status["source_revision"] == coordinate.source,
        "installed readiness named another accepted source"
    );
    let executable = run.home.join(".local/bin/jeden");
    let mut cmd = Command::new(&executable);
    cmd.arg("--version")
        .env("HOME", &run.home)
        .env("STADO_CONFIG", &releases.config);
    let observation = command(run, cmd)?;
    observation.passed()?;
    let version = fs::read_to_string(observation.directory.join("stdout.log"))?;
    let observed = version
        .split_whitespace()
        .nth(1)
        .context("executed Jeden did not report a version")?;
    ensure!(
        observed.split('+').next() == coordinate.version.split('+').next(),
        "executed another release: expected {}, observed {version}",
        coordinate.version
    );
    Ok(())
}

fn journey(run: &mut Run) -> Result<()> {
    let releases = releases::published(run)?;
    let first = install(run, &releases, "install", &releases.older)?;
    ready(run, &releases, &releases.older)?;
    let installed = receipt(run, "installed")?;
    ensure!(
        installed["source_revision"] == releases.older.source,
        "persisted installation has another source"
    );
    let paths = first["installed_paths"]
        .as_array()
        .context("installed file paths")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(PathBuf::from)
                .context("installed path string")
        })
        .collect::<Result<Vec<_>>>()?;
    for path in &paths {
        ensure!(
            path.starts_with(&run.home),
            "test installation escaped its isolated home: {}",
            path.display()
        );
    }
    let binary = run.home.join(".stado/bin/jeden");
    let original_hash = sha256(&binary)?;
    let before = fs::read(state_path(run))?;
    let cmd = product(
        run,
        &releases,
        &[
            "update",
            "jeden",
            "--surface",
            "cli",
            "--release-version",
            &releases.older.version,
            "--source-commit",
            &releases.newer.source,
            "--json",
        ],
    )?;
    command(run, cmd)?.refused()?;
    ensure!(
        sha256(&binary)? == original_hash && fs::read(state_path(run))? == before,
        "a rejected source changed the installed binary or receipt"
    );

    install(run, &releases, "update", &releases.newer)?;
    ready(run, &releases, &releases.newer)?;
    ensure!(
        receipt(run, "updated")?["source_revision"] == releases.newer.source,
        "update did not persist the accepted source"
    );
    let cmd = product(
        run,
        &releases,
        &["rollback", "jeden", "--surface", "cli", "--json"],
    )?;
    command(run, cmd)?.passed()?;
    ensure!(
        sha256(&binary)? == original_hash,
        "rollback did not restore the exact preceding executable"
    );
    ready(run, &releases, &releases.older)?;
    receipt(run, "rolled-back")?;
    let cmd = product(
        run,
        &releases,
        &["remove", "jeden", "--surface", "cli", "--json"],
    )?;
    command(run, cmd)?.passed()?;
    for path in &paths {
        absent(path)?;
    }
    ensure!(
        receipt(run, "removed")?["status"] == "absent",
        "removal did not persist absence"
    );
    let cmd = product(
        run,
        &releases,
        &["status", "jeden", "--surface", "cli", "--json"],
    )?;
    let status = command(run, cmd)?.json()?;
    ensure!(
        status["status"] == "absent" && status["readiness"]["ready"] == false,
        "removed installation remains ready"
    );
    Ok(())
}

fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("observing removal of {}", path.display()))
        }
        Ok(_) => anyhow::bail!("installed path remains after removal: {}", path.display()),
    }
}

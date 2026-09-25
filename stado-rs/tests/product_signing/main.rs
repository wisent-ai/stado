mod credentials;
mod entitlement_merge;
mod policy;
#[path = "../product_support/mod.rs"]
pub mod support;
use anyhow::{ensure, Context, Result};
use credentials::Credentials;
use std::{fs, path::Path, process::Command};
use support::{
    command,
    releases::{self, Coordinate, Releases},
    Run,
};

#[test]
fn real_apple_identity_survives_release_and_policy_changes() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "native Apple signing qualification requires a real Darwin signing host"
    );
    let mut run = Run::new("signing")?;
    let result = journey(&mut run);
    run.finish(result)
}

fn journey(run: &mut Run) -> Result<()> {
    let releases = releases::published(run)?;
    let credentials = Credentials::prepare(run, &releases)?;
    let before = credentials.search_list(run)?;
    let result = exercise(run, &releases, &credentials);
    let restored = credentials.search_list(run).and_then(|after| {
        ensure!(after == before, "signing changed the account's keychain search list; actual before and after commands are retained");
        Ok(())
    });
    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Err(restored)) => {
            Err(error.context(format!("keychain restoration also failed: {restored:#}")))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
    }
}

fn execute(run: &mut Run, path: &Path, coordinate: &Coordinate) -> Result<()> {
    let mut execute = Command::new(path);
    execute.arg("--version").env("HOME", &run.home);
    let observed = command(run, execute)?;
    observed.passed()?;
    let version = fs::read_to_string(observed.directory.join("stdout.log"))?;
    let actual = version
        .split_whitespace()
        .nth(1)
        .context("signed executable did not report a version")?;
    ensure!(
        actual.split('+').next() == coordinate.version.split('+').next(),
        "signing did not preserve the release executable: expected {}, observed {version}",
        coordinate.version
    );
    Ok(())
}

fn exercise(run: &mut Run, releases: &Releases, credentials: &Credentials) -> Result<()> {
    releases::install(run, releases, "install", &releases.older)?;
    let installed = run.home.join(".stado/bin/jeden");
    let preceding = run.root.join("preceding-jeden");
    fs::copy(&installed, &preceding)?;
    let original = policy::inspect(run, &preceding)?;
    ensure!(
        original["state"] == "stable",
        "the genuine published input has no stable Apple identity: {original}"
    );
    let identifier = original["identifier"]
        .as_str()
        .context("published code identifier")?;
    let (older_file, older_entitlements) = policy::requested(run, &preceding, "older")?;
    let mut strip = Command::new("/usr/bin/codesign");
    strip.arg("--remove-signature").arg(&preceding);
    command(run, strip)?.passed()?;
    let signed = policy::sign(run, credentials, &preceding, identifier, None, &older_file)?;
    policy::verify(run, &preceding, &original, &older_entitlements)?;
    execute(run, &preceding, &releases.older)?;
    fs::write(
        run.evidence.join("preceding-signature.json"),
        serde_json::to_vec_pretty(&signed)?,
    )?;

    releases::install(run, releases, "update", &releases.newer)?;
    let candidate = run.root.join("candidate-jeden");
    fs::copy(&installed, &candidate)?;
    let (newer_file, newer_entitlements) = policy::requested(run, &candidate, "newer")?;
    let replacement = policy::sign(
        run,
        credentials,
        &candidate,
        identifier,
        Some(&preceding),
        &newer_file,
    )?;
    policy::verify(run, &candidate, &signed, &newer_entitlements)?;
    execute(run, &candidate, &releases.newer)?;
    fs::write(
        run.evidence.join("replacement-signature.json"),
        serde_json::to_vec_pretty(&replacement)?,
    )?;
    credentials.accept_duplicate_issuers(run, &candidate, identifier)?;
    policy::verify(run, &candidate, &signed, &newer_entitlements)?;
    entitlement_merge::exercise(run, credentials, &candidate, &signed, &newer_entitlements)?;
    policy::refusals(run, credentials, &candidate, identifier)?;
    credentials.refuse_wrong_key(run, &candidate, identifier)?;
    execute(run, &candidate, &releases.newer)?;
    let remove = releases::product(
        run,
        releases,
        &["remove", "jeden", "--surface", "cli", "--json"],
    )?;
    command(run, remove)?.passed()?;
    ensure!(
        !installed.try_exists()?,
        "the isolated release installation remains after removal"
    );
    Ok(())
}

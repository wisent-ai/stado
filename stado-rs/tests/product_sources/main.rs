//! `stado product cargo` against a real Git checkout in an isolated workspace:
//! it records the checkout's identity before Cargo runs and reads it again
//! afterwards, keeps the operator's staged index, and refuses a checkout that
//! is not on `main` without switching its branch.

#[path = "../product_support/mod.rs"]
pub mod support;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{fs, path::Path, process::Command};
use support::{command, Run};

#[test]
fn cargo_attests_the_canonical_checkout_and_refuses_another_branch() -> Result<()> {
    let mut run = Run::new("sources")?;
    let result = journey(&mut run);
    run.finish(result)
}

fn git(run: &mut Run, root: &Path, arguments: &[&str]) -> Result<String> {
    let mut git = Command::new("git");
    git.args(arguments)
        .current_dir(root)
        .env("HOME", &run.home)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let observed = command(run, git)?;
    observed.passed()?;
    Ok(fs::read_to_string(observed.directory.join("stdout.log"))?
        .trim()
        .to_owned())
}

fn journey(run: &mut Run) -> Result<()> {
    let workspace = run.root.join("workspace");
    let checkout = workspace.join("sample");
    fs::create_dir_all(checkout.join("src"))?;
    // The run lives beneath the Stado checkout's `target/`; like any
    // canonical checkout, the sample is its own workspace root.
    fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )?;
    fs::write(
        checkout.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"sample\"\nversion = \"0.1.0\"\n",
    )?;
    // Like every canonical checkout, the sample ignores the build tree where
    // the operation keeps its private scratch.
    fs::write(checkout.join(".gitignore"), "/.build/\n/target/\n")?;
    fs::write(checkout.join("src/lib.rs"), "pub fn sample() {}\n")?;
    git(run, &checkout, &["init", "--initial-branch", "main"])?;
    git(
        run,
        &checkout,
        &["config", "user.email", "product-sources@wisent.com"],
    )?;
    git(
        run,
        &checkout,
        &["config", "user.name", "Product sources journey"],
    )?;
    git(
        run,
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/wisent-ai/sample.git",
        ],
    )?;
    git(run, &checkout, &["add", "."])?;
    git(run, &checkout, &["commit", "--message", "sample"])?;
    let head = git(run, &checkout, &["rev-parse", "HEAD"])?;
    // Staged work the operation must leave exactly as it found it.
    fs::write(checkout.join("src/lib.rs"), "pub fn sample() -> u8 { 1 }\n")?;
    git(run, &checkout, &["add", "src/lib.rs"])?;
    let index = checkout.join(".git/index");
    let staged = fs::read(&index)?;

    let manifest = checkout.join("Cargo.toml");
    let manifest = manifest.to_str().context("manifest path")?.to_owned();
    let mut cargo = run.product(&["cargo", "--manifest-path", &manifest, "--json", "metadata"]);
    cargo.env("WISENT_WORKSPACE", &workspace);
    let report = command(run, cargo)?.json()?;
    ensure!(
        report["state"] == "succeeded" && report["command_exit_status"] == 0,
        "canonical Cargo did not succeed: {report}"
    );
    let evidence = Path::new(report["evidence"].as_str().context("evidence path")?);
    let sources: Value = serde_json::from_slice(&fs::read(evidence.join("sources.json"))?)?;
    let recorded = &sources[0];
    ensure!(
        recorded["revision"] == format!("{head}-dirty"),
        "the staged change was not recorded as the checkout's identity: {recorded}"
    );
    let after: Value =
        serde_json::from_slice(&fs::read(evidence.join("sources/0/source-after.json"))?)?;
    ensure!(
        &after == recorded,
        "the identity read after Cargo differs from the one read before it: {after}"
    );
    ensure!(
        fs::read(&index)? == staged,
        "source inspection changed the staged index"
    );

    git(run, &checkout, &["switch", "--create", "topic"])?;
    let mut refused = run.product(&["cargo", "--manifest-path", &manifest, "--json", "metadata"]);
    refused.env("WISENT_WORKSPACE", &workspace);
    let refused = command(run, refused)?;
    refused.refused()?;
    let stderr = fs::read_to_string(refused.directory.join("stderr.log"))?;
    ensure!(
        stderr.contains("is not on main; no branch was switched"),
        "the refusal did not say the checkout is off main: {stderr}"
    );
    ensure!(
        git(run, &checkout, &["branch", "--show-current"])? == "topic",
        "a refused operation switched the checkout's branch"
    );
    Ok(())
}

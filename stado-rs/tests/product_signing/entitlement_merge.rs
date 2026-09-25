use super::{credentials::Credentials, policy};
use crate::support::{command, Run};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use stado_product::common::sha256;
use std::{fs, path::Path};

const JIT: &str = "com.apple.security.cs.allow-jit";
const DYLD: &str = "com.apple.security.cs.allow-dyld-environment-variables";

fn merge(
    run: &mut Run,
    credentials: &Credentials,
    paths: &[&Path],
    values: &[String],
) -> Result<()> {
    let mut cmd = run.product(&["signing", "sign", "--product", "jeden", "--json"]);
    cmd.args(paths);
    for value in values {
        cmd.arg("--boolean-entitlement").arg(value);
    }
    credentials.configure(&mut cmd);
    command(run, cmd)?.passed()?;
    Ok(())
}

fn changed(base: &plist::Value, key: &str, value: bool) -> Result<plist::Value> {
    let mut expected = base.clone();
    expected
        .as_dictionary_mut()
        .context("the signed entitlement dictionary is absent")?
        .insert(key.into(), plist::Value::Boolean(value));
    Ok(expected)
}

pub fn exercise(
    run: &mut Run,
    credentials: &Credentials,
    target: &Path,
    preceding: &Value,
    original: &plist::Value,
) -> Result<()> {
    let other = run.root.join("merge-peer-jeden");
    fs::copy(target, &other)?;
    merge(run, credentials, &[&other], &[format!("{JIT}=false")])?;
    let other_before = changed(original, JIT, false)?;
    policy::verify(run, &other, preceding, &other_before)?;

    merge(
        run,
        credentials,
        &[target, &other],
        &[format!("{DYLD}=false")],
    )?;
    policy::verify(run, target, preceding, &changed(original, DYLD, false)?)?;
    policy::verify(
        run,
        &other,
        preceding,
        &changed(&other_before, DYLD, false)?,
    )?;

    merge(
        run,
        credentials,
        &[target],
        &[format!("{JIT}=true"), format!("{DYLD}=true")],
    )?;
    policy::verify(run, target, preceding, original)?;

    let before = sha256(target)?;
    for values in [
        vec![format!("{JIT}=enabled")],
        vec![format!("{JIT}=true"), format!("{JIT}=false")],
    ] {
        let mut cmd = run.product(&["signing", "sign", "--product", "jeden", "--json"]);
        cmd.arg(target);
        for value in values {
            cmd.arg("--boolean-entitlement").arg(value);
        }
        credentials.configure(&mut cmd);
        command(run, cmd)?.refused()?;
        ensure!(
            sha256(target)? == before,
            "a refused entitlement request replaced the signed executable"
        );
    }
    policy::verify(run, target, preceding, original)?;
    Ok(())
}

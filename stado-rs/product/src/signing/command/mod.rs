pub mod notarize;
pub mod stage;
use super::{
    core::{identifier, inspect, SignatureState},
    signer::Signer,
    Policy,
};
use crate::{
    common::{emit, Arguments, Runtime},
    paths,
    state::ProductState,
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// The `stado product signing` operations, parsed from the subcommand name.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Residue,
    Notarize,
    Stage,
    Inspect,
    Report,
    Reconcile,
    Sign,
}

pub fn run(name: &str, args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let action = Action::deserialize(Value::String(name.to_owned()))
        .with_context(|| format!("unknown signing action {name}"))?;
    let args = Arguments::from_matches(args);
    if action == Action::Residue {
        let roots = if args.many("--root").is_empty() {
            vec![
                runtime.home.join(".local/bin"),
                runtime.home.join(".stado/bin"),
            ]
        } else {
            args.many("--root").iter().map(PathBuf::from).collect()
        };
        let report = paths::residue(&roots, runtime)?;
        let failed = report.iter().any(|row| !row["recorded"].is_null());
        emit(&json!({"residue": report}))?;
        return Ok(i32::from(failed));
    }
    let reports = if action == Action::Notarize {
        notarize::notarize(
            Path::new(args.required("--app")?),
            args.optional("--evidence")?.map(Path::new),
        )?
    } else if action == Action::Stage {
        stage::stage(
            Path::new(args.required("--manifest")?),
            Path::new(args.required("--output")?),
            args.required("--platform")?,
        )?
    } else if action == Action::Inspect {
        if args.positional.is_empty() {
            bail!("signing inspect requires at least one path");
        }
        if args.has("--entitlements") && !cfg!(target_os = "macos") {
            bail!("reading signed entitlements requires a Darwin host");
        }
        args.positional
            .iter()
            .map(|path| {
                let path = Path::new(path);
                let mut report = inspect(path)?;
                if args.has("--entitlements") {
                    match super::policy::entitlements(path) {
                        Ok(value) => report["entitlements"] = serde_json::to_value(value)?,
                        Err(error) => report["entitlements_error"] = json!(format!("{error:#}")),
                    }
                }
                Ok(report)
            })
            .collect::<Result<Vec<_>>>()?
    } else if matches!(action, Action::Report | Action::Reconcile) {
        if args.positional.len() != 1 {
            bail!("signing {name} requires one product");
        }
        let product = &args.positional[0];
        let surface = args.optional("--surface")?.unwrap_or("cli");
        let state = ProductState::load(runtime, product, surface)?
            .context("product has no recorded installation")?;
        let mut seen = BTreeSet::new();
        let mut reports = Vec::new();
        for path in state.installed_paths {
            let path = path.canonicalize().unwrap_or(path);
            if !seen.insert(path.clone()) {
                continue;
            }
            let report = inspect(&path)?;
            if action == Action::Reconcile && !acceptable(&report) {
                reports.push(super::sign(
                    &path,
                    &identifier(
                        product,
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .context("missing filename")?,
                    )?,
                    None,
                )?);
            } else {
                reports.push(report);
            }
        }
        reports
    } else if action == Action::Sign {
        let product = args.optional("--product")?;
        let code_id = args.optional("--identifier")?;
        let previous = args.optional("--previous")?.map(Path::new);
        if product.is_none() && code_id.is_none() {
            bail!("sign requires --product or --identifier");
        }
        if args.positional.is_empty() {
            bail!("sign requires at least one target");
        }
        if (code_id.is_some() || previous.is_some()) && args.positional.len() != 1 {
            bail!("--identifier and --previous require exactly one target");
        }
        let policy = Policy::new(
            args.optional("--entitlements")?.map(Path::new),
            args.has("--hardened-runtime"),
            args.many("--boolean-entitlement"),
        )?;
        let mut signer = Signer::new(args.optional("--identity")?)?;
        let result = (|| {
            let mut reports = Vec::new();
            for target in &args.positional {
                let path = Path::new(target);
                let before = inspect(previous.filter(|p| p.exists()).unwrap_or(path))?;
                let identifier = match code_id {
                    Some(id) => id.to_owned(),
                    None if before["state"] == "stable" => before["identifier"]
                        .as_str()
                        .context("stable signature has no identifier")?
                        .to_owned(),
                    None => identifier(
                        product.context("missing product")?,
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .context("missing target name")?,
                    )?,
                };
                reports.push(signer.sign(path, &identifier, previous, &policy)?);
            }
            Ok(reports)
        })();
        let cleanup = signer.close();
        match (result, cleanup) {
            (Ok(reports), Ok(())) => reports,
            (Err(error), Err(cleanup)) => {
                return Err(error.context(format!(
                    "signing credential cleanup also failed: {cleanup:#}"
                )))
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
        }
    } else {
        bail!("signing {name} has no operation here");
    };
    let failed = reports
        .iter()
        .any(|row| !acceptable(row) || !row["entitlements_error"].is_null());
    emit(&json!(reports))?;
    Ok(i32::from(failed))
}

/// Whether an installation may keep a target `inspect` reported on; a report
/// whose state is not one `inspect` writes is not.
pub fn acceptable(report: &Value) -> bool {
    SignatureState::of(report).is_ok_and(SignatureState::acceptable)
}

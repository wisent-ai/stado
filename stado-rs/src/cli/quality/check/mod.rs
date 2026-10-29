//! `stado quality check`: the declared gates run over the committed tree an
//! install would build, exported beside the checkout, so the checkout is not
//! written and a refusal names the gate.

mod admitted;
mod command;
mod commands;
mod modules;
pub use command::{check, command_tree};
mod paths;
mod placed;
pub(crate) use placed::{check_bundle, place};
mod secrets;
pub(crate) use modules::check_source;
use std::path::Path;

use super::{
    declared_gates, inputs, lockfile, run, web, Declaration, Diagnostics, Report, Selection,
};
use crate::cli::CmdError;
use crate::release_pipeline::GateKind;

/// Run the lock and the selected gates over `revision` of `checkout`,
/// exported beside it, writing nothing to the checkout, with every release
/// input the manifest pins staged beside the export as the release worker
/// stages it, serving no named defect (`check_revision_for`).
pub(crate) async fn check_revision(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
) -> Result<(), CmdError> {
    check_revision_for(checkout, revision, report, selection, &[]).await
}

/// [`check_revision`] serving `purposes`, the open defects named to Oko's
/// admission before a compile or test gate runs.
pub(crate) async fn check_revision_for(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
    purposes: &[&str],
) -> Result<(), CmdError> {
    export_check(checkout, revision, report, selection, false, purposes).await
}

async fn export_check(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
    if_declared: bool,
    purposes: &[&str],
) -> Result<(), CmdError> {
    // The gates run in the exported tree with that tree as their working
    // directory, and a gate such as `stado web quality` reads the tree from
    // WISENT_SOURCE_DIR. A checkout named relatively (`--source .`) made that
    // path relative to the caller's directory, so the gate looked for
    // `./.wisent-output/quality/...` inside the tree itself and refused with
    // "not a directory". The scratch is named from the absolute checkout.
    let checkout = &std::path::absolute(checkout).map_err(|error| {
        CmdError::click(format!(
            "cannot resolve the checkout {}: {error}",
            checkout.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    // Each run owns one directory holding the exported tree and its staged
    // inputs, as a release job owns its work area. A `--link-input` lands
    // beside the tree (the tree's parent is the work area it may write), so
    // with the tree exported straight into one shared directory every run's
    // link landed there, outlived the run, and refused every later run with
    // "already exists". The run areas live in the checkout's build area under
    // Stado's home, never in the checkout: the checkouts sit in
    // `~/Documents`, which macOS keeps from the janitor, so a run a killed
    // process left there was one no pass could ever reclaim.
    let runs = stado_product::common::runs::checkout_area(
        &crate::config_file::expand_tilde("~"),
        checkout,
    )
    .map_err(|error| CmdError::click(format!("{error:#}")))?
    .join("quality");
    report.say(&format!(
        "stado quality check: reclaiming abandoned quality runs for {revision} in {}",
        runs.display()
    ));
    stado_product::common::runs::reclaim_abandoned(&runs, revision, "stado quality check");
    let run_area = runs.join(format!("{}-{revision}", std::process::id()));
    let scratch = run_area.join("source");
    report.say(&format!(
        "stado quality check: exporting committed revision {revision} from {} into {}",
        checkout.display(),
        scratch.display()
    ));
    let sha256 = stado_product::export_committed_source(checkout, revision, &scratch)
        .map_err(|error| CmdError::unreachable(format!("cannot export {revision}: {error:#}")))?;
    let inputs_area = run_area.join("inputs");
    let verdict = check_tree(
        &scratch,
        &inputs_area,
        checkout,
        &Exported {
            revision,
            sha256: &sha256,
            purposes,
        },
        report,
        selection,
        if_declared,
    )
    .await;
    match std::fs::remove_dir_all(&run_area) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(
                CmdError::click(format!("cannot remove {}: {error}", run_area.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind())),
            )
        }
    }
    let Some(product) = verdict? else {
        return Ok(());
    };
    report.say(&format!(
        "stado quality check: {product} resolves its locks, passes its quality gates and finds its \
         release inputs stored at {revision} of {}",
        checkout.display()
    ));
    Ok(())
}

/// The committed revision a check runs over and the SHA-256 of the archive it
/// was exported from: the release worker states both to a build
/// (`WISENT_SOURCE_COMMIT`, `WISENT_SOURCE_SHA256`), and a product's build
/// reads them together. `purposes` are the open defects the check serves,
/// named to Oko's admission before a compile or test gate runs.
struct Exported<'a> {
    revision: &'a str,
    sha256: &'a str,
    purposes: &'a [&'a str],
}

/// The gates of the tree at `tree`, with its pinned inputs staged under
/// `inputs_area` first; answers the product.
async fn check_tree(
    tree: &Path,
    inputs_area: &Path,
    checkout: &Path,
    exported: &Exported<'_>,
    report: Report,
    selection: Selection,
    if_declared: bool,
) -> Result<Option<String>, CmdError> {
    let revision = exported.revision;
    report.say(&format!(
        "stado quality check: reading declared gates of {revision} in {}",
        tree.display()
    ));
    let declared = match declared_gates(Some(&tree.to_string_lossy()), selection)? {
        Declaration::Gates(declared) => *declared,
        Declaration::Undeclared(reason) if if_declared => {
            report.say(&format!(
                "stado quality check: {revision}: {reason}; nothing to check"
            ));
            return Ok(None);
        }
        Declaration::Undeclared(reason) => return Err(CmdError::refused(reason)),
    };
    report.say(&format!(
        "stado quality check: checking locked dependencies of {revision} in {}",
        tree.display()
    ));
    lockfile::check(tree, checkout, revision, report)?;
    report.say(&format!(
        "stado quality check: checking source modules of {revision} in {}",
        tree.display()
    ));
    modules::check(tree, checkout, revision)?;
    // A pin whose object was never stored, or was stored for a lock the
    // product has since moved past, is refused here, while the session that
    // pushed it can still repair it, instead of by the build that fetches it;
    // a gate that reads an input finds it where the worker would put it.
    report.say(&format!(
        "stado quality check: staging the pinned release inputs of {revision} into {}",
        inputs_area.display()
    ));
    let staged = inputs::stage(&declared, inputs_area, report).await?;
    // A gate reads the secrets the worker would put in its environment, such
    // as a web install's GITHUB_TOKEN for private dependencies.
    report.say(&format!(
        "stado quality check: resolving the secrets the gates of {} read",
        declared.product
    ));
    let secret = secrets::resolved(&declared.secret_env).await?;
    let contract = match &declared.contract_version {
        Some(version) => web::worker_contract(tree, inputs_area, version, &declared.platform)?,
        None => Vec::new(),
    };
    // The exported tree has no Git checkout, so a gate that binds its
    // evidence to the commit (a real journey's report) reads it from here,
    // and a build reads the archive digest beside it, as the release worker
    // states both to a build.
    let contract: Vec<(&str, String)> = [
        ("WISENT_SOURCE_COMMIT", revision.to_owned()),
        ("WISENT_SOURCE_SHA256", exported.sha256.to_owned()),
    ]
    .into_iter()
    .chain(
        contract
            .iter()
            .map(|(name, value)| (&**name, value.clone())),
    )
    .chain(
        staged
            .iter()
            .chain(secret.iter())
            .map(|(name, value)| (name.as_str(), value.clone())),
    )
    .collect();
    // Cargo builds every check of one checkout with one build directory
    // under that checkout's build area: the export moves with each run, and
    // a build directory named from the workspace path compiled every
    // dependency again on every check. Only the build directory moves; the
    // finished artifacts stay in the tree's own `target`, where the recipe's
    // `stage` reads them. The area is Stado's, where the janitor reaches it,
    // never in the checkout.
    let build_dir = stado_product::common::runs::checkout_area(
        &crate::config_file::expand_tilde("~"),
        checkout,
    )
    .map_err(|error| CmdError::click(format!("{error:#}")))?
    .join("cargo-build");
    let mut contract = contract;
    contract.push((
        "CARGO_BUILD_BUILD_DIR",
        build_dir.to_string_lossy().into_owned(),
    ));
    for gate in &declared.gates {
        report.say(&format!("stado quality check: {}", gate.argv.join(" ")));
        // A format gate compiles nothing; every other kind compiles the tree,
        // and runs only as work the operator's rulings admit (`admitted`).
        let diagnostics = match gate.kind {
            GateKind::Format => Diagnostics::Text,
            _ => Diagnostics::Compiled,
        };
        let product = &declared.product;
        report.say(&format!(
            "stado quality check: asking Oko to admit gate {:?} ({}) of {product} at {revision}",
            gate.name, gate.kind
        ));
        admitted::gate(gate.kind, &gate.name, product, revision, exported.purposes).await?;
        run(&gate.argv, tree, &contract, report, diagnostics).map_err(|error| {
            let refused = error.within(format!(
                "stado quality check: gate {:?} ({}) of {} refuses {revision} of {}",
                gate.name,
                gate.kind,
                declared.product,
                checkout.display()
            ));
            match gate.kind {
                GateKind::Format if declared.web_version.is_some() => refused.also(
                    "web gate: inspect the failed command and output; `stado quality format` does not write web source",
                ),
                GateKind::Format => refused.also("`stado quality format` writes what it reads"),
                GateKind::Lint | GateKind::Test => refused.also(format!(
                    "a {} gate's refusal is repaired in the source it names; `stado quality format` \
                     changes nothing for it",
                    gate.kind
                )),
            }
        })?;
    }
    if matches!(selection, Selection::Installation | Selection::CommandTree) {
        let area = tree.parent().ok_or_else(|| {
            CmdError::click(format!(
                "exported source has no run area: {}",
                tree.display()
            ))
        })?;
        report.say(&format!(
            "stado quality check: asking Oko to admit the build of {} at {revision}",
            declared.product
        ));
        admitted::build(&declared.product, revision, exported.purposes).await?;
        if matches!(selection, Selection::CommandTree) {
            commands::inventory(&declared, tree, area, &contract, revision, report)?;
        } else {
            commands::check(&declared, tree, area, &contract, revision, report)?;
        }
    }
    Ok(Some(declared.product))
}

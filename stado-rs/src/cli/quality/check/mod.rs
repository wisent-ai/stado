//! `stado quality check`: the declared gates run over the committed tree an
//! install would build, exported beside the checkout, so the checkout is not
//! written and a refusal names the gate.

mod command;
mod commands;
mod modules;
pub use command::check;
mod paths;
pub(crate) use modules::check_source;
use std::path::Path;

use super::{declared_gates, inputs, lockfile, run, web, Declaration, Report, Selection};
use crate::cli::CmdError;
use crate::release_pipeline::GateKind;

/// Run the lock and the selected gates over `revision` of `checkout`,
/// exported beside it, writing nothing to the checkout, with every release
/// input the manifest pins staged beside the export as the release worker
/// stages it. Every gate runs to its end; a refusal names all of them.
pub(crate) async fn check_revision(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
) -> Result<(), CmdError> {
    export_check(checkout, revision, report, selection, false).await
}

/// Every refusal of the selected checks over `revision` of `checkout`, each
/// as its check or gate said it, with the gate's own output kept: an empty
/// list is a pass. A check that could not run at all (the export, the
/// manifest, a pinned input) is the error.
pub(crate) async fn refusals(
    checkout: &Path,
    revision: &str,
    selection: Selection,
) -> Result<Vec<String>, CmdError> {
    Ok(
        match exported(checkout, revision, Report::Kept, selection, false).await? {
            Some(checked) => checked.refusals.iter().map(ToString::to_string).collect(),
            None => Vec::new(),
        },
    )
}

/// The product a check read and every refusal of its run.
struct Checked {
    product: String,
    refusals: Vec<CmdError>,
}

async fn export_check(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
    if_declared: bool,
) -> Result<(), CmdError> {
    let Some(checked) = exported(checkout, revision, report, selection, if_declared).await? else {
        return Ok(());
    };
    // A lone refusal is returned as its check said it, with its class; more
    // than one are said together, every one on its own line.
    let mut refusals = checked.refusals.into_iter();
    match (refusals.next(), refusals.next()) {
        (None, _) => {
            report.say(&format!(
                "stado quality check: {} resolves its locks, passes its quality gates and finds \
                 its release inputs stored at {revision} of {}",
                checked.product,
                checkout.display()
            ));
            Ok(())
        }
        (Some(only), None) => Err(only),
        (Some(first), Some(second)) => {
            let said: Vec<String> = [first, second]
                .into_iter()
                .chain(refusals)
                .map(|refusal| refusal.to_string())
                .collect();
            Err(CmdError::refused(format!(
                "{} refusals of {revision} of {}, every check run to its end:\n{}",
                said.len(),
                checkout.display(),
                said.join("\n")
            )))
        }
    }
}

async fn exported(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
    if_declared: bool,
) -> Result<Option<Checked>, CmdError> {
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
    remove_abandoned(&runs);
    let run_area = runs.join(format!(
        "{}-{}",
        std::process::id(),
        &revision[..12.min(revision.len())]
    ));
    let scratch = run_area.join("source");
    stado_product::export_committed_source(checkout, revision, &scratch)
        .map_err(|error| CmdError::unreachable(format!("cannot export {revision}: {error:#}")))?;
    let inputs_area = run_area.join("inputs");
    let verdict = check_tree(
        &scratch,
        &inputs_area,
        checkout,
        revision,
        report,
        selection,
        if_declared,
    )
    .await;
    if run_area.exists() {
        std::fs::remove_dir_all(&run_area).map_err(|error| {
            CmdError::click(format!("cannot remove {}: {error}", run_area.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    verdict
}

/// Remove the run areas under `runs` whose process is gone: a check that was
/// killed never reached its own removal. A run area is named
/// `<pid>-<revision>`; one whose process still lives is another check's, and
/// a removal that fails is said on stderr and leaves the area to a later run.
/// So is a runs directory, or an entry in it, that cannot be read: dropped in
/// silence, abandoned run areas piled up with nothing saying why.
fn remove_abandoned(runs: &Path) {
    let entries = match std::fs::read_dir(runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "stado quality check: cannot list the run areas in {}: {error}",
                runs.display()
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "stado quality check: cannot read a run area in {}: {error}",
                    runs.display()
                );
                continue;
            }
        };
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
        else {
            continue;
        };
        let gone = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            == Err(nix::errno::Errno::ESRCH);
        if gone {
            if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                eprintln!(
                    "stado quality check: cannot remove the abandoned run {}: {error}",
                    entry.path().display()
                );
            }
        }
    }
}

/// The gates of the tree at `tree`, with its pinned inputs staged under
/// `inputs_area` first; answers the product and every refusal. Each check
/// and gate runs to its end whatever the one before it said: stopping at the
/// first refusal is what made every installation find the next one.
async fn check_tree(
    tree: &Path,
    inputs_area: &Path,
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
    if_declared: bool,
) -> Result<Option<Checked>, CmdError> {
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
    let mut refusals = Vec::new();
    if let Err(refused) = lockfile::check(tree, checkout, revision, report) {
        refusals.push(refused);
    }
    if let Err(refused) = modules::check(tree, checkout, revision) {
        refusals.push(refused);
    }
    // A pin whose object was never stored, or was stored for a lock the
    // product has since moved past, is refused here, while the session that
    // pushed it can still repair it, instead of by the build that fetches it;
    // a gate that reads an input finds it where the worker would put it.
    let staged = inputs::stage(&declared, inputs_area, report).await?;
    let contract = match &declared.contract_version {
        Some(version) => web::worker_contract(tree, version, &declared.platform)?,
        None => Vec::new(),
    };
    let contract: Vec<(&str, String)> = contract
        .iter()
        .map(|(name, value)| (&**name, value.clone()))
        .chain(
            staged
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone())),
        )
        .collect();
    for gate in &declared.gates {
        report.say(&format!("stado quality check: {}", gate.argv.join(" ")));
        if let Err(error) = run(&gate.argv, tree, &contract, report) {
            let refused = error.within(format!(
                "stado quality check: gate {:?} ({}) of {} refuses {revision} of {}",
                gate.name,
                gate.kind,
                declared.product,
                checkout.display()
            ));
            // Only a format gate reads what `stado quality format` writes; a
            // lint or test gate's refusal is repaired in the source it names.
            refusals.push(match gate.kind {
                GateKind::Format => refused.also("`stado quality format` writes what it reads"),
                GateKind::Lint | GateKind::Test => refused.also(format!(
                    "a {} gate's refusal is repaired in the source it names; `stado quality format` \
                     changes nothing for it",
                    gate.kind
                )),
            });
        }
    }
    if matches!(selection, Selection::Installation) {
        let area = tree.parent().ok_or_else(|| {
            CmdError::click(format!(
                "exported source has no run area: {}",
                tree.display()
            ))
        })?;
        if let Err(refused) = commands::check(&declared, tree, area, revision, &contract, report) {
            refusals.push(refused);
        }
    }
    Ok(Some(Checked {
        product: declared.product,
        refusals,
    }))
}

//! `stado quality check`: the declared gates run over the committed tree an
//! install would build, exported beside the checkout, so the checkout is not
//! written and a refusal names the gate.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    declared_gates, inputs, lockfile, run, selected_gates, web, Declaration, Report, Selection,
};
use crate::cli::CmdError;
use crate::release_pipeline::GateKind;

/// Run the gates an installation runs (every declared kind) over the
/// committed tree an install of the checkout at `root` would build, writing
/// nothing to the checkout; the first refusal is returned with the gate that
/// refused. The pre-push check is this, so a pushed commit is checked by the
/// same gates the installation of it runs.
///
/// `stado product install|update` exports the committed revision and runs its
/// quality there, so an uncommitted edit neither fails nor rescues it. The
/// check reads that same tree: `origin/main` when the tracked tree is clean
/// and behind it (the install advances to it), `HEAD` otherwise.
///
/// With `if_declared`, a manifest that declares no quality gate (or
/// `releases:false`) is an answer, not a refusal: the sentence saying so is
/// printed and nothing is checked. The pre-push dispatcher asks this instead
/// of reading the manifest's text itself.
pub async fn check(root: Option<&str>, if_declared: bool) -> Result<(), CmdError> {
    if if_declared {
        if let Declaration::Undeclared(reason) = declared_gates(root, Selection::Installation)? {
            println!("{reason}; nothing to check");
            return Ok(());
        }
    }
    let checkout = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?,
    };
    let revision = built_revision(&checkout)?;
    check_revision(
        &checkout,
        &revision,
        Report::Stdout,
        Selection::Installation,
    )
    .await
}

/// Run the lock and the selected gates over `revision` of `checkout`,
/// exported beside it, writing nothing to the checkout, with every release
/// input the manifest pins staged beside the export as the release worker
/// stages it.
pub(crate) async fn check_revision(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
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
    )
    .await;
    if run_area.exists() {
        std::fs::remove_dir_all(&run_area).map_err(|error| {
            CmdError::click(format!("cannot remove {}: {error}", run_area.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let product = verdict?;
    report.say(&format!(
        "stado quality check: {product} resolves its locks, passes its quality gates and finds its \
         release inputs stored at {revision} of {}",
        checkout.display()
    ));
    Ok(())
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
/// `inputs_area` first; answers the product.
async fn check_tree(
    tree: &Path,
    inputs_area: &Path,
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
) -> Result<String, CmdError> {
    lockfile::check(tree, checkout, revision, report)?;
    let declared = selected_gates(Some(&tree.to_string_lossy()), selection)?;
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
        run(&gate.argv, tree, &contract, report).map_err(|error| {
            let refused = error.within(format!(
                "stado quality check: gate {:?} ({}) of {} refuses {revision} of {}",
                gate.name,
                gate.kind,
                declared.product,
                checkout.display()
            ));
            // Only a format gate reads what `stado quality format` writes; a
            // lint or test gate's refusal is repaired in the source it names.
            match gate.kind {
                GateKind::Format => refused.also("`stado quality format` writes what it reads"),
                GateKind::Lint | GateKind::Test => refused.also(format!(
                    "a {} gate's refusal is repaired in the source it names; `stado quality format` \
                     changes nothing for it",
                    gate.kind
                )),
            }
        })?;
    }
    Ok(declared.product)
}

/// The revision an install of `checkout` builds: `origin/main` after a fetch
/// when the tracked tree is clean and `HEAD` is contained in it, else `HEAD`.
fn built_revision(checkout: &Path) -> Result<String, CmdError> {
    let head = git(checkout, &["rev-parse", "HEAD"])?;
    let dirty = !git(checkout, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    if dirty {
        return Ok(head);
    }
    git(checkout, &["fetch", "--quiet", "origin", "main"])?;
    let main = git(checkout, &["rev-parse", "origin/main"])?;
    let contained = crate::wait::status(
        Command::new("git")
            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
            .current_dir(checkout),
    )
    .map_err(|error| {
        CmdError::click(format!("cannot run git merge-base: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?
    .success();
    Ok(if contained { main } else { head })
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = crate::wait::output(Command::new("git").args(args).current_dir(checkout))
        .map_err(|error| {
            CmdError::click(format!("cannot run git {}: {error}", args.join(" ")))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

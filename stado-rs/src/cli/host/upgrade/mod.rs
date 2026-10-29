//! `stado host upgrade TARGET` — bring a host's installed Stado to a newer
//! committed revision, for a host whose own Stado is too old to follow the
//! release path: one that cannot read the registry authority, or holds no
//! checkout to build from.
//!
//! Everything a host's own Stado would need is done from here: the revision
//! is fetched here, compiled on a builder of the host's platform
//! (`stado host cargo`), signed here with the stable identity, delivered into
//! a run of its own on the host (`stado host run deliver`), judged by the
//! incoming program against the host's configuration and installed by it
//! (`release local install`, through `stado host run attach`). What answered
//! before the install is read again after it; a check that no longer answers
//! puts the kept previous program back (`release local restore
//! --kept-previous`). Every step reports what it found, and `--plan` stops
//! after the reads.

mod apply;
mod checks;
mod identity;
mod install;
mod installed;
mod reads;
mod report;
mod source;

use std::io::IsTerminal as _;

use serde_json::json;

use crate::cli::CmdError;

use report::{Report, State};
use source::Admitted;

/// What `stado host upgrade` was given on its command line.
pub struct UpgradeRequest {
    pub target: String,
    pub source_commit: Option<String>,
    pub builder: Option<String>,
    pub signing_keychain: Option<String>,
    pub signing_identity: Option<String>,
    pub join_network: bool,
    pub plan: bool,
    pub json: bool,
}

/// `stado host upgrade`: run the upgrade and print its report.
pub async fn upgrade(request: UpgradeRequest) -> Result<(), CmdError> {
    let mut report = Report::new(&request.target, request.plan, request.json);
    let result = run(&request, &mut report).await;
    report.outcome = match &result {
        Ok(outcome) => outcome.clone(),
        Err(error) => format!("failed: {error}"),
    };
    if request.json {
        let mut document = serde_json::to_value(&report)?;
        if let Err(error) = &result {
            document["error"] = json!(error.to_string());
        }
        crate::cli::host::checks::probes::print_json(&document);
        return match result {
            Ok(_) => Ok(()),
            Err(error) => Err(CmdError::silent(error.code)),
        };
    }
    result.map(|outcome| println!("{}: {outcome}", request.target))
}

async fn run(request: &UpgradeRequest, report: &mut Report) -> Result<String, CmdError> {
    let runner = crate::deploy::production_runner();
    let target = crate::cli::canonical_host(&request.target).await?;
    let source = source::resolve(request.source_commit.as_deref()).await?;
    report.say(
        "source",
        State::Ok,
        format!(
            "{} (Stado {}), origin/main is {}",
            source.commit, source.version, source.origin_main
        ),
    );
    report.source = Some(serde_json::to_value(&source)?);
    let installed = installed::read(&target, &runner).await?;
    report.installed = Some(serde_json::to_value(&installed)?);
    let kept = match &installed.kept {
        Some(kept) => format!("{} (sha256 {})", kept.path, kept.sha256),
        None => "none kept yet".to_string(),
    };
    report.say(
        "installed",
        State::Ok,
        format!(
            "{} at {} (sha256 {}, signature {}); rollback target: {kept}",
            installed.version_line,
            installed.program.path,
            installed.program.sha256,
            installed.program.signature.state
        ),
    );
    if let Admitted::Current = source::admit(&source, &installed, &target.name).await? {
        report.say(
            "install",
            State::Skipped,
            format!("{} already runs {}", target.name, source.commit),
        );
        return Ok(format!("already runs {}; nothing to do", source.commit));
    }
    let builder = reads::builder(request.builder.as_deref(), &target).await?;
    let planned = if request.plan {
        State::Planned
    } else {
        State::Ok
    };
    report.say(
        "builder",
        planned,
        format!(
            "{} compiles {} for {}",
            builder.name, source.commit, target.release_platform
        ),
    );
    let choice = stado_product::SigningChoice::new(
        request.signing_keychain.as_deref(),
        request.signing_identity.as_deref(),
    )
    .map_err(|error| CmdError::usage(format!("{error:#}")))?;
    let identity = identity::predict(&choice, &installed)?;
    report.identity = Some(serde_json::to_value(&identity)?);
    let verdict = if identity.change {
        "the code identity changes"
    } else {
        "unchanged"
    };
    report.say(
        "identity",
        State::Ok,
        format!("{verdict}: {}", identity.why),
    );
    let mut notes = Vec::new();
    let dialogs = identity::dialogs(
        &target,
        &installed,
        &identity,
        request.join_network,
        &runner,
        &mut notes,
    )
    .await?;
    for note in notes {
        report.say("dialogs", State::Failed, note);
    }
    for dialog in &dialogs {
        report.say(
            format!("dialog on {}", dialog.host),
            State::Planned,
            format!(
                "{}: “{}” → {} ({})",
                dialog.when, dialog.says, dialog.answer, dialog.because
            ),
        );
    }
    if dialogs.is_empty() {
        report.say("dialogs", State::Ok, "no macOS dialog is expected");
    }
    report.dialogs = dialogs;
    let (state, said) = reads::declarations(&target, &runner).await;
    report.say("declarations", state, said);
    let before = checks::run(&target, false, &runner).await;
    report.checks("before", &before);
    report.checks_before = before;
    let (state, said) = reads::gates(&target.name, &runner).await;
    report.say("gates", state, said);
    if request.plan {
        if identity.change {
            report.say(
                "terminal",
                State::Planned,
                format!(
                    "the identity changes, so the upgrade runs only from a terminal, with \
                     someone at {} to answer the dialogs above",
                    target.name
                ),
            );
        }
        for step in [
            "build",
            "sign",
            "deliver",
            "validate",
            "install",
            "checks after",
        ] {
            report.say(step, State::Planned, "not run: --plan");
        }
        return Ok(format!(
            "planned {} → {}; nothing was changed",
            installed.version_line, source.commit
        ));
    }
    if identity.change {
        if !std::io::stdin().is_terminal() {
            return Err(CmdError::refused(format!(
                "{} would change code identity ({}), so macOS will ask the person at {} the \
                 questions listed above; this upgrade runs with no terminal for anyone to see \
                 them; run it from a terminal with someone at {}; nothing was changed",
                target.name, identity.why, target.name, target.name
            )));
        }
        if !reads::confirmed(&target.name).await? {
            return Err(CmdError::refused(format!(
                "the host's name was not typed back, so the upgrade of {} did not start; \
                 nothing was changed",
                target.name
            )));
        }
    }
    let built = apply::build(&builder, &source, installed.darwin, &runner).await?;
    report.say(
        "build",
        State::Ok,
        format!("{} (sha256 {})", built.path.display(), built.sha256),
    );
    let area = apply::work_area(&source)?;
    let install = install::Install {
        target: &target,
        source: &source,
        installed: &installed,
        identity: &identity,
        choice: &choice,
        built: &built,
        request,
    };
    let applied = install::deliver_and_install(&install, &area, report, &runner).await;
    match apply::remove_area(&area) {
        Ok(said) => report.say("cleanup here", State::Ok, said),
        Err(said) => report.say("cleanup here", State::Failed, said),
    }
    applied
}

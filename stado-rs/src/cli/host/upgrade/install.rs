//! The changing half of an upgrade: sign and package the built program,
//! deliver it, let it judge and install itself on the host, read the checks
//! again, and put the kept program back when one stopped answering.

use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use super::apply::{self, Built, Delivered};
use super::checks::{self, Check};
use super::identity::Identity;
use super::installed::{self, Installed};
use super::report::{Report, State};
use super::source::Source;
use super::UpgradeRequest;

/// What the install is given: the facts the plan read, and the program.
pub(super) struct Install<'a> {
    pub target: &'a ComputeTarget,
    pub source: &'a Source,
    pub installed: &'a Installed,
    pub identity: &'a Identity,
    pub choice: &'a stado_product::SigningChoice,
    pub built: &'a Built,
    pub request: &'a UpgradeRequest,
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

/// Sign, package and deliver the program, install it, and remove the run.
pub(super) async fn deliver_and_install(
    install: &Install<'_>,
    area: &std::path::Path,
    report: &mut Report,
    runner: &Runner,
) -> Result<String, CmdError> {
    let (target, installed) = (install.target, install.installed);
    let sign = installed
        .darwin
        .then_some((install.choice, install.identity.identifier.as_str()));
    let (program, signed) = apply::place(install.built, area, sign)?;
    let announced = match &signed {
        Some(signed) => {
            let Some(after) = signed["requirement"].as_str() else {
                return Err(CmdError::unreachable(format!(
                    "{} was signed but names no designated requirement: {signed}",
                    program.display()
                )));
            };
            report.say(
                "sign",
                State::Ok,
                format!("{} carries {after}", program.display()),
            );
            let changes = installed.program.signature.requirement.as_deref() != Some(after);
            if changes && !install.identity.change {
                return Err(CmdError::refused(format!(
                    "the signed program carries {after}, not the installed {:?} the plan read as \
                     unchanged, so macOS would ask at {} questions nobody was told about; \
                     nothing was delivered",
                    installed.program.signature.requirement, target.name
                )));
            }
            changes.then(|| after.to_string())
        }
        None => {
            report.say("sign", State::Skipped, format!("{} is no Mac", target.name));
            None
        }
    };
    let archive_sha256 = apply::package(area, &program)?;
    report.say("package", State::Ok, format!("sha256 {archive_sha256}"));
    let delivered = apply::deliver(target, area, &installed.home, runner).await?;
    report.say("deliver", State::Ok, delivered.run_directory.clone());
    let result = install_and_check(
        install,
        &delivered,
        &archive_sha256,
        announced,
        report,
        runner,
    )
    .await;
    match apply::remove_run(target, &delivered, runner).await {
        Ok(said) => report.say("cleanup on host", State::Ok, said),
        Err(said) => report.say("cleanup on host", State::Failed, said),
    }
    result
}

async fn install_and_check(
    install: &Install<'_>,
    delivered: &Delivered,
    archive_sha256: &str,
    announced: Option<String>,
    report: &mut Report,
    runner: &Runner,
) -> Result<String, CmdError> {
    let (target, source, installed) = (install.target, install.source, install.installed);
    let capture = install.request.json;
    let validated = apply::attach(target, delivered, &words(&["config", "validate"]), true).await?;
    if let Err(why) = apply::ended(&validated) {
        report.say("validate", State::Failed, why.clone());
        return Err(CmdError::refused(format!(
            "the incoming Stado refuses {}'s configuration ({why}); nothing was installed",
            target.name
        )));
    }
    report.say(
        "validate",
        State::Ok,
        "the incoming Stado accepts the host's configuration",
    );
    let mut arguments = words(&["release", "local", "install", "--archive"]);
    arguments.extend([
        delivered.archive.clone(),
        "--sha256".to_string(),
        archive_sha256.to_string(),
    ]);
    if let Some(requirement) = announced {
        arguments.extend(["--announced-identity".to_string(), requirement]);
    }
    let outcome = apply::attach(target, delivered, &arguments, capture).await?;
    if let Err(why) = apply::ended(&outcome) {
        report.say("install", State::Failed, why.clone());
        return Err(CmdError::unreachable(format!(
            "release local install on {} {why}; an install that fails after placing the program \
             puts its predecessor back itself",
            target.name
        )));
    }
    report.say(
        "install",
        State::Ok,
        format!("{} installed from {}", source.commit, delivered.archive),
    );
    let now = installed::read(target, runner).await?;
    let revision = |installed: &Installed, answered: bool| Check {
        name: "installed revision".to_string(),
        answered,
        detail: installed.version_line.clone(),
    };
    // The install may have moved a unit (an agent into the system domain on
    // a headless Mac) and recorded it from the host, so the after checks read
    // the declarations as they are now: the authority snapshot this machine
    // kept predates that write.
    crate::cli::resolver::forget_recalled_registry();
    let declared_now = crate::cli::canonical_host(&target.name).await?;
    let mut after = checks::run(&declared_now, true, runner).await;
    let runs_source = now.revision.as_deref() == Some(source.commit.as_str()) && !now.dirty;
    after.insert(0, revision(&now, runs_source));
    report.checks("after", &after);
    report.checks_after = after.clone();
    let mut before = report.checks_before.clone();
    before.insert(0, revision(installed, true));
    let regressed: Vec<String> = checks::regressed(&before, &after)
        .into_iter()
        .map(|check| format!("{}: {}", check.name, check.detail))
        .collect();
    if !regressed.is_empty() {
        return Err(roll_back(install, delivered, &regressed, report, runner).await);
    }
    let (state, said) = super::reads::gates(&target.name, runner).await;
    report.say("gates", state, said);
    if install.request.join_network {
        if let Err(error) = crate::cli::network::up_host(&target.name).await {
            report.say("network", State::Failed, error.to_string());
            return Err(error.within(format!(
                "{} runs {}, and joining Stado's network failed",
                target.name, source.commit
            )));
        }
        report.say(
            "network",
            State::Ok,
            format!("{} joined Stado's network", target.name),
        );
    }
    if now.darwin {
        privacy_log(target, report, runner).await;
    }
    Ok(format!("runs {} ({})", source.commit, now.version_line))
}

/// Put the kept program back with the delivered one's `release local
/// restore --kept-previous`, read the host again, and answer the failure
/// that says what stopped answering and whether the rollback held.
async fn roll_back(
    install: &Install<'_>,
    delivered: &Delivered,
    regressed: &[String],
    report: &mut Report,
    runner: &Runner,
) -> CmdError {
    let (target, installed) = (install.target, install.installed);
    let mut arguments = words(&["release", "local", "restore", "--kept-previous"]);
    arguments.push(installed.program.sha256.clone());
    let restored = match apply::attach(target, delivered, &arguments, install.request.json).await {
        Ok(outcome) => apply::ended(&outcome),
        Err(error) => Err(error.to_string()),
    };
    let detail = match (restored, installed::read(target, runner).await) {
        (Ok(()), Ok(back)) if back.program.sha256 == installed.program.sha256 => {
            report.rolled_back = true;
            format!("{} runs the kept {} again", target.name, back.version_line)
        }
        (Ok(()), Ok(back)) => format!(
            "the restore exited 0, but {} runs sha256 {} ({}), not the kept {}",
            target.name, back.program.sha256, back.version_line, installed.program.sha256
        ),
        (Ok(()), Err(error)) => {
            format!("the restore exited 0, and the host's Stado was not read again: {error}")
        }
        (Err(why), _) => format!("release local restore --kept-previous {why}"),
    };
    let state = if report.rolled_back {
        State::Ok
    } else {
        State::Failed
    };
    report.say("rollback", state, detail.clone());
    CmdError::unreachable(format!(
        "after the install on {} these no longer answered: {}; {}",
        target.name,
        regressed.join("; "),
        if report.rolled_back {
            format!("rolled back: {detail}")
        } else {
            format!("the rollback did not complete: {detail}")
        }
    ))
}

/// The last hour of `tccd`'s log lines naming stado, through the
/// allowlisted host read: the questions macOS put after the install.
async fn privacy_log(target: &ComputeTarget, report: &mut Report, runner: &Runner) {
    let (_, said, receipt) = checks::exec(
        &target.name,
        crate::deploy::host_exec::MACOS_TCC_LOG_READ,
        runner,
    )
    .await;
    let Some(stdout) = receipt["stdout"].as_str() else {
        report.say("privacy log", State::Failed, said);
        return;
    };
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains("stado"))
        .collect();
    let detail = if lines.is_empty() {
        "tccd logged nothing naming stado in the last hour".to_string()
    } else {
        lines.join(" | ")
    };
    report.say("privacy log", State::Ok, detail);
}

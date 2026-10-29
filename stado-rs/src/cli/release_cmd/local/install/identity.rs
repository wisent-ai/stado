//! The code identity macOS keeps a program's privacy decisions under, judged
//! before the program is placed.
//!
//! macOS stores every decision — Documents, Desktop, Downloads, Photos, Local
//! Network — under the program's designated requirement. A program placed
//! with another requirement is a stranger to it, and the next time the
//! running host process reaches for a declared need macOS asks the person at
//! the keyboard again, from a background process nobody is watching. So a
//! program must carry one stable identity, and a change of identity is made
//! only with someone at a terminal, told first which questions follow.

use std::path::Path;

use crate::cli::CmdError;

/// The designated requirement of `program`, or the signature's state when
/// it carries none.
fn requirement(program: &Path, name: &str) -> Result<String, CmdError> {
    let report = stado_product::inspect_signature(program).map_err(|error| {
        CmdError::click(format!(
            "the signature of {name} at {} cannot be read: {error:#}",
            program.display()
        ))
    })?;
    Ok(match report["requirement"].as_str() {
        Some(requirement) => requirement.to_string(),
        None => format!("a signature in state {}", report["state"]),
    })
}

/// Refuse a native program whose signature is not one stable Apple identity
/// (`stado_product::signature_acceptable`), naming its state and the command
/// that installs a signed release.
pub(super) fn refuse_unstable(candidate: &Path, name: &str, archive: &str) -> Result<(), CmdError> {
    let report = stado_product::inspect_signature(candidate).map_err(|error| {
        CmdError::click(format!(
            "the signature of {name} from {archive} cannot be read: {error:#}"
        ))
    })?;
    if stado_product::signature_acceptable(&report) {
        return Ok(());
    }
    let state = match report["state"].as_str() {
        Some(state) => state.to_string(),
        None => report["state"].to_string(),
    };
    let detail = match report["error"].as_str() {
        Some(detail) => format!(": {detail}"),
        None => String::new(),
    };
    Err(CmdError::refused(format!(
        "{name} from {archive} is not signed with a stable identity ({state}{detail}); macOS \
         keeps its privacy decisions per signing identity, so it would be a new program there \
         and ask the person at the keyboard every question again; install a signed release \
         with `stado product install {name} --source-commit <commit on origin/main>`"
    )))
}

/// On a Mac, the candidate replacing an installed program under another
/// designated requirement: refused with no terminal (a delivery, a host
/// channel), and with one the questions macOS will ask again are said first.
pub(super) fn refuse_changed(
    installed: &Path,
    candidate: &Path,
    name: &str,
) -> Result<(), CmdError> {
    use std::io::IsTerminal as _;
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let present = crate::primitives::presence::present(installed).map_err(|error| {
        CmdError::click(format!("{}: {error}", installed.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if !present {
        return Ok(());
    }
    let (before, after) = (requirement(installed, name)?, requirement(candidate, name)?);
    if before == after {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(CmdError::refused(format!(
            "{name} would change code identity ({before} → {after}), so macOS would ask the \
             person at this Mac again for every privacy decision it keeps for {} \
             (`targets[].privacy_grants`, `stado host privacy <this host>`), from the background \
             process that runs it; this install runs with no terminal for anyone to see why; run \
             it at this Mac",
            installed.display()
        )));
    }
    eprintln!(
        "{name}: the installed program changes code identity ({before} → {after}); macOS will \
         ask on this Mac again for each privacy decision it keeps for {} — the ones \
         `targets[].privacy_grants` declares, listed by `stado host privacy <this host>` — the \
         next time the host process reaches for it; allow the declared ones",
        installed.display()
    );
    Ok(())
}

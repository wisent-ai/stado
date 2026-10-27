//! `stado resolver remembered show|forget [--json]`: what this machine
//! remembers about reading its remote registry authority, and dropping it.
//!
//! Both records end by themselves once the refresh interval passes. An
//! operator who has just repaired the authority's host or this host's
//! network, or moved the authority, need not wait for that: `forget` drops
//! the remembered given-up read and the kept snapshot, so the next command
//! asks the authority at once.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// The registry authority this host's last-known-good copy names, or `None`
/// when it keeps no copy or the copy declares no service directory.
fn authority() -> Result<Option<String>, CmdError> {
    let Some(copy) = super::super::document::kept_last_good_document()
        .map_err(CmdError::declaration)?
    else {
        return Ok(None);
    };
    Ok(crate::service_resolution::directory(&copy)
        .map_err(CmdError::declaration)?
        .map(|directory| directory.authority.target))
}

fn report(authority: &str) -> Value {
    let kept = super::recalled::kept(authority)
        .map(|(version, until)| json!({ "version": version, "until": until.to_rfc3339() }));
    json!({
        "authority": authority,
        "unanswered": super::unanswered::standing(authority),
        "kept_snapshot": kept,
    })
}

fn print(document: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(document)?);
        return Ok(());
    }
    crate::cli::print_answer(document, false)
}

/// `show`: the remembered given-up read and the kept snapshot of the
/// authority this host's copy names, each `null` when none stands.
pub(in crate::cli::resolver) fn show(json_output: bool) -> Result<(), CmdError> {
    let document = match authority()? {
        Some(authority) => report(&authority),
        None => json!({
            "authority": Value::Null,
            "detail": "this host keeps no last-known-good registry copy naming an authority, so nothing about reading one is remembered",
        }),
    };
    print(&document, json_output)
}

/// `forget`: drop both records, so the next command asks the authority. What
/// stood before is printed, so the operator sees what was dropped.
pub(in crate::cli::resolver) fn forget(json_output: bool) -> Result<(), CmdError> {
    let Some(authority) = authority()? else {
        return show(json_output);
    };
    let before = report(&authority);
    super::unanswered::forget(&authority);
    super::recalled::forget();
    print(&json!({ "forgot": before }), json_output)
}

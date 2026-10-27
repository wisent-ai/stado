//! Which vault this machine's credential operations resolve to, and which
//! keys on this machine can still open it.

use serde_json::{json, Value};

use crate::cli::{reporting::table, CmdError};

use crate::cli::secrets::store::resolve::skarbiec_binary;

/// `stado credentials vault show [--json]` — the resolution itself, reported.
///
/// The same rule the fleet sweep applies to another host's report is applied
/// here to this machine's own candidates, so the two cannot answer
/// differently: `stado credentials vault list --host <target>` and this command state one
/// verdict in one vocabulary.
///
/// It also names the host that owns the fleet vault, the registry's
/// `skarbiec` active host. A different host declaring a local copy in
/// `secrets.skarbiec.vault_file` is refused: that copy diverges from the
/// owner (grants minted there never reach it), and it leaves that host
/// holding a Skarbiec of its own.
pub(crate) async fn vault_authority(json_output: bool) -> Result<(), CmdError> {
    let candidates =
        crate::credential_store::owner::candidates_present().map_err(CmdError::from)?;
    let declared = crate::config::skarbiec_vault_file();
    let verdict = crate::credential_store::owner::authority(Some(declared), &candidates);
    let mut state = match verdict.get("state").and_then(serde_json::Value::as_str) {
        Some(state) => state.to_string(),
        None => {
            return Err(CmdError::declaration(format!(
                "the vault authority verdict carries no state, so who may hold the vault cannot be told: {verdict}"
            )))
        }
    };
    // The resolver is asked as well as described: a report that computed the
    // state itself and never called `vault()` could agree with nothing.
    let mut resolved = crate::credential_store::owner::vault().map_err(|error| error.to_string());
    let owner_host = crate::cli::directory::active_host("skarbiec").await?;
    let this_host = crate::cli::release_catalog::this_host().await;
    if let (Some(owner), Err(error)) = (&owner_host, &this_host) {
        // Without this host's name the owner check below cannot run, and
        // skipping it judged a non-owner's local files as if it owned the vault.
        state = "host_unreadable".to_string();
        resolved = Err(format!(
            "which registry host this machine is could not be read ({error}), so whether it \
             may hold the fleet vault owned by {owner} cannot be judged"
        ));
    }
    if let (Some(owner), Ok(here)) = (&owner_host, &this_host) {
        if owner != here && !declared.trim().is_empty() {
            state = "local_copy_on_non_owner".to_string();
            resolved = Err(format!(
                "{here} declares a local vault copy ({declared}) in secrets.skarbiec.vault_file, \
                 but the fleet vault is owned by {owner} (the registry's skarbiec active host); \
                 read the owner through secrets.skarbiec.url and remove \
                 secrets.skarbiec.vault_file"
            ));
        } else if owner != here {
            // A non-owner that declares no local copy is the configuration
            // the refusal above asks for. Judging its leftover local files
            // afterwards would answer `ambiguous` and tell it to declare one
            // of them again, one command after
            // `stado config unset secrets.skarbiec.vault_file`.
            state = "reads_owner".to_string();
            resolved = Err(format!(
                "{here} holds no fleet vault: it reads the vault on {owner} through \
                 secrets.skarbiec.url ({}); any vault file on this machine is a retired copy",
                crate::config::skarbiec_url()
            ));
        }
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": state,
                "path": match &resolved {
                    Ok(path) => Some(path.display().to_string()),
                    Err(_) => None,
                },
                "detail": verdict.get("detail"),
                "declared": if declared.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::from(declared)
                },
                "candidates": candidates,
                "owner_host": owner_host,
                "refusal": resolved.as_ref().err(),
            }))?
        );
    } else {
        println!("state: {state}");
        match &resolved {
            Ok(path) => println!("vault: {}", path.display()),
            Err(error) => println!("vault: none — {error}"),
        }
        if !declared.trim().is_empty() {
            println!("declared: {declared}");
        }
        if let Some(owner) = &owner_host {
            println!("fleet vault owner: {owner}");
        }
        for candidate in &candidates {
            // A candidate field the report left out is named; a count of 0
            // and an owner "?" read as values that were measured.
            let items = match candidate.get("items").and_then(serde_json::Value::as_u64) {
                Some(items) => items.to_string(),
                None => crate::cli::reporting::NOT_REPORTED.to_string(),
            };
            let owner = match candidate.get("owner").and_then(serde_json::Value::as_str) {
                Some(owner) => owner,
                None => crate::cli::reporting::NOT_REPORTED,
            };
            let path = match candidate.get("path").and_then(serde_json::Value::as_str) {
                Some(path) => path,
                None => "path not reported",
            };
            println!("  {items:>5} items  owner {owner}  {path}");
        }
    }
    let Err(error) = resolved else {
        return Ok(());
    };
    if state == "reads_owner" {
        return Ok(());
    }
    // With `--json` the document already carries `state` and `refusal`, and a
    // second JSON error printed after it makes the answer unparseable — one
    // report per invocation, and the exit status is the part a script gates on.
    if json_output {
        return Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE));
    }
    Err(CmdError::declaration(error))
}

/// One reader for Skarbiec's verdict, shared by `doctor` and the `harvest`
/// restore guard so the two can never disagree about whether the vault opens.
///
/// It runs Skarbiec's own `keys check` rather than reimplementing the check.
/// The vault and the keyring belong to Skarbiec; a second opinion computed here
/// could disagree with the program that actually performs the decryption, and
/// during an outage two answers are worse than none.
pub(crate) fn key_doctor_report(binary: &std::path::Path) -> Result<Value, CmdError> {
    let output = crate::wait::output(std::process::Command::new(binary).args(["keys", "check"]))?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::click(format!(
            "{} keys check produced no report ({error}): {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

/// Report which keys can still open the vault, and which key files a restore
/// needs when none can.
pub(crate) fn doctor(json: bool) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    let report = key_doctor_report(&binary)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return verdict(&report);
    }
    // A report without its recipients, or a recipient without a field or
    // flag, said nothing about it: printed as an empty table, empty text or
    // "no", a missing secret-half flag read as a key nobody can restore.
    let Some(recipients) = report.get("recipients").and_then(Value::as_array) else {
        println!("the doctor report lists no recipients: {report}");
        return verdict(&report);
    };
    let rows: Vec<Vec<String>> = recipients
        .iter()
        .map(|entry| {
            let field = |name: &str| crate::cli::reporting::field(&entry[name]);
            let flag = |name: &str| match entry[name].as_bool() {
                Some(true) => "yes".to_string(),
                Some(false) => "no".to_string(),
                None => crate::cli::reporting::NOT_REPORTED.to_string(),
            };
            // The encryption subkey file is the one a restore must
            // produce, and Skarbiec lists it last.
            let key_file = match entry["key_files"].as_array().and_then(|files| files.last()) {
                Some(file) => crate::cli::reporting::field(file),
                None => crate::cli::reporting::NOT_REPORTED.to_string(),
            };
            vec![
                field("uid"),
                field("role"),
                flag("is_owner"),
                flag("secret_half_present"),
                key_file,
            ]
        })
        .collect();
    table::print(
        &["RECIPIENT", "ROLE", "DOC OWNER", "SECRET HALF", "KEY FILE"],
        &rows,
    );
    match (
        report.get("vault").and_then(Value::as_str),
        report.get("status").and_then(Value::as_str),
    ) {
        (Some(vault), Some(status)) => println!("vault {vault} is {status}"),
        _ => println!("the doctor report does not name its vault and status: {report}"),
    }
    verdict(&report)
}

/// A readable vault exits zero; anything else is a failure an operator has to
/// see in `$?`, not only on screen.
fn verdict(report: &Value) -> Result<(), CmdError> {
    match report["status"].as_str() {
        Some("readable") | Some("empty") => Ok(()),
        // A report that names no remedy is quoted, so the refusal says what
        // the doctor saw rather than a remedy nobody stated.
        _ => Err(CmdError::click(match report["remedy"].as_str() {
            Some(remedy) => remedy.to_string(),
            None => {
                format!("the vault is not readable here and the doctor names no remedy: {report}")
            }
        })
        .stating(crate::primitives::failure::FailureCode::Auth)),
    }
}

//! Which vault this machine's credential operations resolve to, and which
//! keys on this machine can still open it.

use serde_json::{json, Value};

use crate::cli::{reporting::table, CmdError};

use crate::cli::secrets::store::resolve::skarbiec_binary;

/// `stado credentials vault [--json]` — the resolution itself, reported.
///
/// The same rule the fleet sweep applies to another host's report is applied
/// here to this machine's own candidates, so the two cannot answer
/// differently: `stado host vaults <target>` and this command state one
/// verdict in one vocabulary.
pub(crate) fn vault_authority(json_output: bool) -> Result<(), CmdError> {
    let candidates = crate::credential_store::owner::candidates_present()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let declared = crate::config::skarbiec_vault_file();
    let verdict = crate::credential_store::owner::authority(Some(declared), &candidates);
    let state = verdict
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    // The resolver is asked as well as described: a report that computed the
    // state itself and never called `vault()` could agree with nothing.
    let resolved = crate::credential_store::owner::vault();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": state,
                "path": resolved.as_ref().ok().map(|path| path.display().to_string()),
                "detail": verdict.get("detail"),
                "declared": if declared.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::from(declared)
                },
                "candidates": candidates,
                "refusal": resolved.as_ref().err().map(ToString::to_string),
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
        for candidate in &candidates {
            println!(
                "  {:>5} items  owner {}  {}",
                candidate
                    .get("items")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default(),
                candidate
                    .get("owner")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?"),
                candidate
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            );
        }
    }
    match resolved {
        Ok(_) => Ok(()),
        // With `--json` the document already carries `state` and `refusal`,
        // and a second JSON error printed after it makes the answer
        // unparseable — one report per invocation, and the exit status is the
        // part a script gates on.
        Err(_) if json_output => Err(CmdError::silent(1)),
        Err(error) => Err(CmdError::click(error.to_string())),
    }
}

/// One reader for Skarbiec's verdict, shared by `doctor` and the `harvest`
/// restore guard so the two can never disagree about whether the vault opens.
///
/// It runs Skarbiec's own `key-doctor` rather than reimplementing the check.
/// The vault and the keyring belong to Skarbiec; a second opinion computed here
/// could disagree with the program that actually performs the decryption, and
/// during an outage two answers are worse than none.
pub(crate) fn key_doctor_report(binary: &std::path::Path) -> Result<Value, CmdError> {
    let output = std::process::Command::new(binary)
        .arg("key-doctor")
        .output()?;
    serde_json::from_slice(&output.stdout).map_err(|_| {
        CmdError::click(format!(
            "{} key-doctor produced no report: {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
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
    let rows: Vec<Vec<String>> = report
        .get("recipients")
        .and_then(Value::as_array)
        .map(|recipients| {
            recipients
                .iter()
                .map(|entry| {
                    let field = |name: &str| {
                        entry
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    let flag = |name: &str| match entry
                        .get(name)
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        true => "yes".to_string(),
                        false => "no".to_string(),
                    };
                    // The encryption subkey file is the one a restore must
                    // produce, and Skarbiec lists it last.
                    let key_file = entry
                        .get("key_files")
                        .and_then(Value::as_array)
                        .and_then(|files| files.last())
                        .and_then(Value::as_str)
                        .unwrap_or("(no keygrip)")
                        .to_string();
                    vec![
                        field("uid"),
                        field("role"),
                        flag("is_owner"),
                        flag("secret_half_present"),
                        key_file,
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    table::print(
        &["RECIPIENT", "ROLE", "DOC OWNER", "SECRET HALF", "KEY FILE"],
        &rows,
    );
    println!(
        "vault {} is {}",
        report
            .get("vault")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        report
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
    verdict(&report)
}

/// A readable vault exits zero; anything else is a failure an operator has to
/// see in `$?`, not only on screen.
fn verdict(report: &Value) -> Result<(), CmdError> {
    match report.get("status").and_then(Value::as_str) {
        Some("readable") | Some("empty") => Ok(()),
        _ => Err(CmdError::click(
            report
                .get("remedy")
                .and_then(Value::as_str)
                .unwrap_or("the vault cannot be opened by any key on this machine"),
        )),
    }
}

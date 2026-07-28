//! `stado secrets` — operator surface for the separate Skarbiec service.
//!
//! Secret values travel in request bodies, never argv. Skarbiec performs
//! encryption, authorization, versioning, recovery-recipient handling, and
//! audit logging. Stado retains no local or cloud-secret-manager fallback.

use std::io::Read;

use clap::Subcommand;
use serde_json::{json, Value};

use super::{table, CmdError};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store an item in Skarbiec, reading its value from STDIN.
    Put {
        /// Skarbiec item id.
        name: String,
    },
    /// Print one Skarbiec item value or one exact string field to stdout.
    Get {
        /// Skarbiec item id.
        name: String,
        /// Print only this string field. The item id and field remain separate.
        #[arg(long)]
        field: Option<String>,
    },
    /// List metadata for items authorized by the current grant.
    Ls {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Soft-delete a Skarbiec item.
    Rm {
        /// Skarbiec item id.
        name: String,
    },
    /// Report whether any key on this machine can still open the vault, and
    /// which key files a restore needs when none can.
    Doctor {
        /// Emit Skarbiec's own JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Inventory credentials recoverable from agent transcripts. Reports names
    /// and counts, never values.
    Harvest {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Restore one exact name into the vault, newest observed value first.
        /// The value streams to Skarbiec and is never printed.
        #[arg(long)]
        restore: Option<String>,
        /// Also scan payloads that merely quoted a file. Those are source code,
        /// so their names are usually identifiers, not credentials in use.
        #[arg(long)]
        all: bool,
    },
}

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    match command {
        // `doctor` is answered before any client exists. Every other verb needs
        // a grant, a token and a live service, which is exactly the set of
        // things this verb is for when one of them is what broke.
        SecretsCommands::Doctor { json } => doctor(json),
        // Same reasoning as `doctor`: the transcripts are readable when the
        // vault is not, which is the only reason this verb is worth having.
        SecretsCommands::Harvest { json, restore, all } => harvest(json, restore.as_deref(), all),
        SecretsCommands::Put { name } => put(&client()?, &name).await,
        SecretsCommands::Get { name, field } => get(&client()?, &name, field.as_deref()).await,
        SecretsCommands::Ls { json } => ls(&client()?, json).await,
        SecretsCommands::Rm { name } => rm(&client()?, &name).await,
    }
}

/// Inventory of credentials still recoverable from agent transcripts, and the
/// single-name restore path.
///
/// The report carries names, counts, dates and source files. It carries no
/// values, because the defect it measures is values reaching places that only
/// needed names — printing them here would add a terminal, a shell history and
/// this process's own transcript to that list. `--restore NAME` is the one path
/// a value travels, and it goes straight into Skarbiec's stdin.
fn harvest(json: bool, restore: Option<&str>, all: bool) -> Result<(), CmdError> {
    if let Some(name) = restore {
        let value = crate::transcripts::value_for(name).ok_or_else(|| {
            CmdError::click(format!(
                "no secret-shaped value for {name} in any transcript; run without --restore to see what is there"
            ))
        })?;
        let binary = skarbiec_binary()?;
        // Encrypting needs only public halves, so a write into a vault nobody
        // can open SUCCEEDS and produces one more unreadable item. Refuse: the
        // recovered value would be buried in the same hole it is being pulled
        // out of.
        let report = key_doctor_report(&binary)?;
        match report.get("status").and_then(Value::as_str) {
            Some("readable") | Some("empty") => {}
            _ => {
                return Err(CmdError::click(format!(
                    "refusing to restore {name}: the vault cannot be opened by any key here, so the write would be encrypted to recipients nobody holds. Own a readable vault first (see `stado secrets doctor`)"
                )))
            }
        }
        let mut child = std::process::Command::new(&binary)
            .arg("set")
            .arg(name)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(value.as_bytes())?;
        }
        let finished = child.wait_with_output()?;
        if !finished.status.success() {
            return Err(CmdError::click(format!(
                "{} set {name} failed: {}",
                binary.display(),
                String::from_utf8_lossy(&finished.stderr).trim()
            )));
        }
        println!("restored {name} into the vault from transcript history");
        return Ok(());
    }
    let findings = crate::transcripts::scan(all);
    if json {
        let rendered: Vec<Value> = findings
            .iter()
            .map(|finding| {
                json!({
                    "name": finding.name,
                    "occurrences": finding.occurrences,
                    "distinct_values": finding.distinct_values,
                    "newest_seen": finding.newest_seen,
                    "sources": finding.sources.len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!(rendered))?);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = findings
        .iter()
        .map(|finding| {
            vec![
                finding.name.clone(),
                finding.occurrences.to_string(),
                finding.distinct_values.to_string(),
                finding.newest_seen.clone(),
                finding.sources.len().to_string(),
                match finding.origin {
                    crate::transcripts::Origin::Runtime => "runtime".to_string(),
                    crate::transcripts::Origin::FileQuote => "file".to_string(),
                },
            ]
        })
        .collect();
    table::print(
        &["NAME", "SEEN", "DISTINCT", "NEWEST", "FILES", "ORIGIN"],
        &rows,
    );
    println!(
        "{} recoverable credential name(s) in agent transcripts; restore one with --restore NAME",
        rows.len()
    );
    Ok(())
}

fn client() -> Result<crate::skarbiec::Client, CmdError> {
    crate::skarbiec::Client::configured().map_err(|err| CmdError::click(err.to_string()))
}

/// Where Stado installs Skarbiec, mirroring
/// [`crate::deploy::host_recovery::WC_CANDIDATES`]: one prefix, discovered the
/// same way, so the two cannot drift apart.
const SKARBIEC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec"];

fn skarbiec_binary() -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    // `SKARBIEC_BIN` is the override the credential scripts already use, and it
    // is the only way to diagnose a build before it is installed — which is the
    // situation whenever the installed binary is the thing that is stale.
    if let Ok(explicit) = std::env::var("SKARBIEC_BIN") {
        let path = std::path::PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(CmdError::click(format!(
                "SKARBIEC_BIN names no file: {explicit}"
            )));
        }
        return Ok(path);
    }
    for candidate in SKARBIEC_CANDIDATES {
        let path = std::path::PathBuf::from(candidate.replace("$HOME", &home));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(CmdError::click(format!(
        "no installed skarbiec binary at {}",
        SKARBIEC_CANDIDATES.join(", ")
    )))
}

/// One reader for Skarbiec's verdict, shared by `doctor` and the `harvest`
/// restore guard so the two can never disagree about whether the vault opens.
///
/// It runs Skarbiec's own `key-doctor` rather than reimplementing the check.
/// The vault and the keyring belong to Skarbiec; a second opinion computed here
/// could disagree with the program that actually performs the decryption, and
/// during an outage two answers are worse than none.
fn key_doctor_report(binary: &std::path::Path) -> Result<Value, CmdError> {
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
fn doctor(json: bool) -> Result<(), CmdError> {
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

fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

async fn put(vault: &crate::skarbiec::Client, name: &str) -> Result<(), CmdError> {
    let input = read_value_from_stdin()?;
    if input.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the value in (stado secrets put NAME < file)",
        ));
    }
    let value = serde_json::from_str(&input).unwrap_or_else(|_| json!({"value": input}));
    vault
        .write_item(name, "stado-secret", &value)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("stored Skarbiec item {name:?}");
    Ok(())
}

async fn get(
    vault: &crate::skarbiec::Client,
    name: &str,
    field: Option<&str>,
) -> Result<(), CmdError> {
    let value = vault
        .read_item(name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if let Some(field) = field {
        let raw = value
            .as_object()
            .and_then(|object| object.get(field))
            .and_then(Value::as_str)
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "Skarbiec item {name:?} has no non-empty string field {field:?}"
                ))
            })?;
        println!("{raw}");
        return Ok(());
    }
    if let Some(object) = value.as_object() {
        if object.len() == usize::from(true) {
            if let Some(raw) = object.get("value").and_then(Value::as_str) {
                println!("{raw}");
                return Ok(());
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn ls(vault: &crate::skarbiec::Client, as_json: bool) -> Result<(), CmdError> {
    let stored = vault
        .list_items()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&stored)?);
        return Ok(());
    }
    if stored.is_empty() {
        println!("No Skarbiec items are visible to this grant.");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = stored
        .iter()
        .map(|item| {
            vec![
                item.id.clone(),
                item.item_type.clone().unwrap_or_else(unknown),
                item.updated_at
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_else(unknown),
                item.versions
                    .map(|versions| versions.to_string())
                    .unwrap_or_else(unknown),
            ]
        })
        .collect();
    table::print(&["NAME", "TYPE", "UPDATED", "VERSIONS"], &rows);
    Ok(())
}

fn unknown() -> String {
    "-".to_string()
}

async fn rm(vault: &crate::skarbiec::Client, name: &str) -> Result<(), CmdError> {
    vault
        .delete_item(name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("soft-deleted Skarbiec item {name:?}");
    Ok(())
}

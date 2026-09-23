//! `stado secrets` — operator surface for the selected credential store.
//!
//! Secret values travel in request bodies, never argv. The backend selected by
//! `STADO_CREDENTIALS_STORE` owns every item; changing it is completed through
//! the verified `migrate` command before normal credential access resumes.

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use clap::Subcommand;
use serde_json::{json, Value};

use super::{table, CmdError};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store an item in the selected credential store, reading from STDIN.
    Put {
        /// Credential item id.
        name: String,
    },
    /// Print one credential item value or one exact string field to stdout.
    Get {
        /// Credential item id.
        name: String,
        /// Print only this string field. The item id and field remain separate.
        #[arg(long)]
        field: Option<String>,
    },
    /// List metadata for items visible to the credential-store admin.
    Ls {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Delete one item from the selected credential store.
    Rm {
        /// Credential item id.
        name: String,
    },
    /// Move every credential to a new backend and commit the selector.
    Migrate {
        /// Destination selector. Omit when STADO_CREDENTIALS_STORE changed.
        #[arg(long)]
        to: Option<String>,
    },
    /// Mint one request-only bootstrap token directly into an owner-only file.
    #[command(name = "mint-acquisition-token")]
    MintAcquisitionToken {
        /// Exact consumer identity.
        consumer: String,
        /// Exact existing Skarbiec item id.
        item: String,
        /// Exact string field the consumer may request.
        field: String,
        /// New token file. Refuses to overwrite an existing path.
        output: String,
    },
    /// Report whether any key on this machine can still open the vault, and
    /// which key files a restore needs when none can.
    Doctor {
        /// Emit Skarbiec's own JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List nonsecret item metadata from one owner-controlled local vault file.
    #[command(name = "inspect-vault")]
    InspectVault {
        /// Encrypted Skarbiec vault file.
        vault: String,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Recreate Weles internal authorities from surviving owner credentials.
    #[command(name = "bootstrap-weles")]
    BootstrapWeles {
        /// Emit only the recreated item names as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inventory credentials recoverable from agent transcripts. Reports names
    /// and counts, never values.
    Harvest {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Restore one exact name into the selected store, newest observation first.
        /// The value is never printed.
        #[arg(long)]
        restore: Option<String>,
        /// Also scan payloads that merely quoted a file. Those are source code,
        /// so their names are usually identifiers, not credentials in use.
        #[arg(long)]
        all: bool,
    },
    /// Test unlock phrases found in transcripts against the vault, reporting
    /// which source name worked. Never prints a phrase.
    TryUnlock {},
}

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    match command {
        // `doctor` is answered before any client exists. Every other verb needs
        // a grant, a token and a live service, which is exactly the set of
        // things this verb is for when one of them is what broke.
        SecretsCommands::Doctor { json } => doctor(json),
        SecretsCommands::InspectVault { vault, json } => inspect_vault(&vault, json),
        SecretsCommands::BootstrapWeles { json } => bootstrap_weles(json),
        // Same reasoning as `doctor`: the transcripts are readable when the
        // vault is not, which is the only reason this verb is worth having.
        SecretsCommands::Harvest { json, restore, all } => {
            harvest(json, restore.as_deref(), all).await
        }
        // Also answered without a client: a protected key that nothing can
        // unlock is precisely the state where every other verb is unavailable.
        SecretsCommands::TryUnlock {} => try_unlock(),
        SecretsCommands::Migrate { to } => migrate(to.as_deref()).await,
        SecretsCommands::Put { name } => put(&client()?, &name).await,
        SecretsCommands::Get { name, field } => get(&client()?, &name, field.as_deref()).await,
        SecretsCommands::Ls { json } => ls(&client()?, json).await,
        SecretsCommands::Rm { name } => rm(&client()?, &name).await,
        SecretsCommands::MintAcquisitionToken {
            consumer,
            item,
            field,
            output,
        } => mint_acquisition_token(&consumer, &item, &field, &output),
    }
}

/// Inventory of credentials still recoverable from agent transcripts, and the
/// single-name restore path.
///
/// The report carries names, counts, dates and source files. It carries no
/// values, because the defect it measures is values reaching places that only
/// needed names — printing them here would add a terminal, a shell history and
/// this process's own transcript to that list. `--restore NAME` is the one path
/// a value travels, and it writes directly to the selected credential store.
async fn harvest(json: bool, restore: Option<&str>, all: bool) -> Result<(), CmdError> {
    if let Some(name) = restore {
        let value = crate::transcripts::value_for(name).ok_or_else(|| {
            CmdError::click(format!(
                "no secret-shaped value for {name} in any transcript; run without --restore to see what is there"
            ))
        })?;
        if crate::credential_store::configured_selector()
            .map_err(|error| CmdError::click(error.to_string()))?
            .starts_with("skarbiec")
        {
            let binary = skarbiec_binary()?;
            // Skarbiec can encrypt with public recipients even when no owner
            // here can decrypt. Refuse to bury the recovered value in that
            // state; other backends enforce their own write preconditions.
            let report = key_doctor_report(&binary)?;
            match report.get("status").and_then(Value::as_str) {
                Some("readable") | Some("empty") => {}
                _ => {
                    return Err(CmdError::click(format!(
                        "refusing to restore {name}: Skarbiec cannot be opened by any key here; own a readable vault first (see `stado secrets doctor`)"
                    )))
                }
            }
        }
        client()?
            .write_item(name, "stado-secret", &json!({"value": value}))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        println!("restored {name} into the selected credential store from transcript history");
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
                    // The table shows this; automation needs it too, or it
                    // cannot tell a live credential from a committed literal.
                    "origin": match finding.origin {
                        crate::transcripts::Origin::Runtime => "runtime",
                        crate::transcripts::Origin::FileQuote => "file",
                    },
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
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|err| CmdError::click(err.to_string()))?;
    crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|err| CmdError::click(err.to_string()))
}

async fn migrate(destination: Option<&str>) -> Result<(), CmdError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|err| CmdError::click(err.to_string()))?;
    let report = crate::credential_store::migrate::migrate(
        destination,
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .await
    .map_err(|err| CmdError::click(err.to_string()))?;
    println!(
        "migrated {} credential item(s): {} -> {}",
        report.moved_items, report.source, report.destination
    );
    Ok(())
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

const SKARBIEC_LAUNCHER_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec-keychain-launcher"];

fn skarbiec_launcher() -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    if let Ok(explicit) = std::env::var("SKARBIEC_LAUNCHER") {
        let path = std::path::PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(CmdError::click(format!(
                "SKARBIEC_LAUNCHER names no file: {explicit}"
            )));
        }
        return Ok(path);
    }
    for candidate in SKARBIEC_LAUNCHER_CANDIDATES {
        let path = std::path::PathBuf::from(candidate.replace("$HOME", &home));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(CmdError::click(format!(
        "no installed Skarbiec launcher at {}",
        SKARBIEC_LAUNCHER_CANDIDATES.join(", ")
    )))
}

fn exact_component(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(CmdError::click(format!(
            "{kind} must be a non-empty exact name containing only ASCII letters, digits, dot, underscore, or dash"
        )));
    }
    Ok(())
}

fn mint_acquisition_token(
    consumer: &str,
    item: &str,
    field: &str,
    output: &str,
) -> Result<(), CmdError> {
    exact_component("consumer", consumer)?;
    exact_component("item", item)?;
    exact_component("field", field)?;
    let output_path = std::path::Path::new(output);
    if output_path.try_exists()? {
        return Err(CmdError::click(format!(
            "refusing to overwrite existing token file {}",
            output_path.display()
        )));
    }
    let launcher = skarbiec_launcher()?;
    let scope = format!("{item}#{field}");
    let minted = std::process::Command::new(&launcher)
        .arg("token-mint")
        .arg(consumer)
        .arg("--acquisition-scopes")
        .arg(&scope)
        .output()?;
    if !minted.status.success() {
        return Err(CmdError::click(format!(
            "{} token-mint failed: {}",
            launcher.display(),
            String::from_utf8_lossy(&minted.stderr).trim()
        )));
    }
    let report: Value = serde_json::from_slice(&minted.stdout).map_err(|_| {
        CmdError::click(format!(
            "{} token-mint produced no JSON report",
            launcher.display()
        ))
    })?;
    let token = report
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Skarbiec token-mint report contained no token"))?;
    let owner_read_write = (u8::BITS - u16::BITS / u8::BITS) << (u8::BITS - u16::BITS / u8::BITS);
    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(owner_read_write)
            .open(output_path)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::process::Command::new(&launcher)
            .arg("token-revoke")
            .arg(consumer)
            .output();
        let _ = std::fs::remove_file(output_path);
        return Err(CmdError::click(format!(
            "cannot write token file {}: {error}; the freshly minted grant was revoked",
            output_path.display()
        )));
    }
    println!(
        "minted request-only {scope} grant for {consumer} into {}",
        output_path.display()
    );
    Ok(())
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

fn inspect_vault(path: &str, json: bool) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(path)?;
    let unsafe_bits = u32::from_str_radix("077", u8::BITS).unwrap_or_default();
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & unsafe_bits != u32::default()
    {
        return Err(CmdError::click(
            "vault must be an owner-only regular local file",
        ));
    }
    let launcher = skarbiec_launcher()?;
    let output = std::process::Command::new(&launcher)
        .arg("list")
        .arg("--all")
        .env("SKARBIEC_VAULT_FILE", path)
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect {}: {}",
            launcher.display(),
            path,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let items: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|_| CmdError::click("Skarbiec inventory was not a JSON array"))?;
    let grants_output = std::process::Command::new(&launcher)
        .arg("tokens")
        .env("SKARBIEC_VAULT_FILE", path)
        .output()?;
    if !grants_output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect grants in {}: {}",
            launcher.display(),
            path,
            String::from_utf8_lossy(&grants_output.stderr).trim()
        )));
    }
    let grants: Vec<Value> = serde_json::from_slice(&grants_output.stdout)
        .map_err(|_| CmdError::click("Skarbiec grant inventory was not a JSON array"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "vault": path,
                "items": items,
                "count": items.len(),
                "grants": grants,
            }))?
        );
        return Ok(());
    }
    let rows = items
        .iter()
        .map(|item| {
            vec![
                item.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or_default()
                    .to_string(),
            ]
        })
        .collect::<Vec<Vec<String>>>();
    table::print(&["NAME", "TYPE", "UPDATED", "DELETED"], &rows);
    println!(
        "{} item(s), {} grant(s) in {}",
        items.len(),
        grants.len(),
        path
    );
    Ok(())
}

fn launcher_json(
    binary: &std::path::Path,
    vault: &std::path::Path,
    arguments: &[&str],
) -> Result<Value, CmdError> {
    let output = std::process::Command::new(binary)
        .args(arguments)
        .env("SKARBIEC_VAULT_FILE", vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} {} failed: {}",
            binary.display(),
            arguments.first().copied().unwrap_or("command"),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| CmdError::click("Skarbiec returned a malformed local JSON report"))
}

fn generated_authority(
    binary: &std::path::Path,
    vault: &std::path::Path,
) -> Result<String, CmdError> {
    launcher_json(
        binary,
        vault,
        &[
            "generate", "--length", "64", "--lower", "--upper", "--digits",
        ],
    )?
    .get("password")
    .and_then(Value::as_str)
    .filter(|secret| !secret.is_empty())
    .map(str::to_string)
    .ok_or_else(|| CmdError::click("Skarbiec generator returned no authority value"))
}

fn store_local_json(
    binary: &std::path::Path,
    vault: &std::path::Path,
    item: &str,
    value: &Value,
) -> Result<(), CmdError> {
    let mut child = std::process::Command::new(binary)
        .arg("set-json")
        .arg(item)
        .arg("--type")
        .arg("internal-authority")
        .env("SKARBIEC_VAULT_FILE", vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(serde_json::to_string(value)?.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "Skarbiec could not store {item}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn bootstrap_weles(json_output: bool) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let vault = std::path::Path::new(&home)
        .join(".stado")
        .join("weles-skarbiec.vault.json");
    if !vault.try_exists()? {
        let output = std::process::Command::new(&binary)
            .arg("init")
            .arg("weles-skarbiec-owner")
            .env("SKARBIEC_VAULT_FILE", &vault)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE")
            .output()?;
        if !output.status.success() {
            return Err(CmdError::click(format!(
                "Skarbiec could not initialize the Weles recovery vault: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    let database_role = crate::transcripts::value_for("WELES_SUPABASE_SERVICE_ROLE_KEY")
        .ok_or_else(|| {
            CmdError::click(
                "WELES_SUPABASE_SERVICE_ROLE_KEY is not recoverable from incident history",
            )
        })?;
    let operator_token =
        crate::transcripts::value_for("WELES_CONSOLE_API_TOKEN").ok_or_else(|| {
            CmdError::click("WELES_CONSOLE_API_TOKEN is not recoverable from incident history")
        })?;
    let model_router_token = generated_authority(&binary, &vault)?;
    let database_url = "https://rbqjqnouluslojmmnuqi.supabase.co";
    let agent_id = "weles";
    let items = vec![
        (
            "weles-database",
            json!({"url": database_url, "service_role_key": database_role}),
        ),
        (
            "weles-object-api",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        ("weles-model-router", json!({"token": model_router_token})),
        (
            "weles-model-agent-auth",
            json!({
                "id": agent_id,
                "agent_auth_secret": generated_authority(&binary, &vault)?,
            }),
        ),
        (
            "weles-artifact-delivery",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-artifact-signing",
            json!({"signing_secret": generated_authority(&binary, &vault)?}),
        ),
        (
            "oko-weles-subscriptions",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-content-diagnostics",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-trading-tools-ingest",
            json!({
                "token": generated_authority(&binary, &vault)?,
                "hmac_secret": generated_authority(&binary, &vault)?,
            }),
        ),
        (
            "weles-operator-cdp",
            json!({
                "url": "http://127.0.0.1:8788",
                "token": operator_token,
            }),
        ),
        (
            "echo-weles-api",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-keyword-planner-model-router",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
    ];
    let mut stored = Vec::with_capacity(items.len());
    for (item, value) in &items {
        store_local_json(&binary, &vault, item, value)?;
        stored.push(*item);
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "recreated",
                "vault": vault,
                "items": stored,
            }))?
        );
    } else {
        println!(
            "recreated {} Weles internal authority item(s)",
            stored.len()
        );
    }
    Ok(())
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

/// Test every unlock phrase the transcripts still hold against the vault.
///
/// The oracle is Skarbiec's own `key-doctor`, run once per candidate with the
/// phrase in its environment: if the canary item opens, that phrase is the one.
/// Reusing the existing verdict means no second decryption path and no crypto
/// written here.
///
/// Reports the SOURCE NAME of the phrase that worked, never the phrase. A
/// passphrase that leaked into a transcript should not also be printed to a
/// terminal by the tool that found it.
fn try_unlock() -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    let candidates = crate::transcripts::unlock_candidates();
    if candidates.is_empty() {
        return Err(CmdError::click(
            "no unlock phrase of any kind survives in transcript runtime output",
        ));
    }
    println!(
        "testing {} distinct phrase(s) from transcript history",
        candidates.len()
    );
    for (name, phrase) in &candidates {
        let output = std::process::Command::new(&binary)
            .arg("key-doctor")
            .env("SKARBIEC_UNLOCK", phrase)
            .output()?;
        let report: Value = match serde_json::from_slice(&output.stdout) {
            Ok(report) => report,
            Err(_) => continue,
        };
        if let Some("readable") = report.get("status").and_then(Value::as_str) {
            println!("the vault OPENS with the phrase recorded under {name}");
            println!("set it as SKARBIEC_UNLOCK, then rotate-owner onto a key you control");
            return Ok(());
        }
    }
    Err(CmdError::click(format!(
        "none of the {} surviving phrase(s) opens the vault: the protected key's passphrase is not in any transcript",
        candidates.len()
    )))
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
    println!("stored credential item {name:?}");
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
                    "credential item {name:?} has no non-empty string field {field:?}"
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
        println!("No credential items are visible to this store administrator.");
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
    println!("removed credential item {name:?}");
    Ok(())
}

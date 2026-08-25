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
        /// Canonical Skarbiec kind. Defaults to the payload's own `kind` when
        /// stdin carries one, else `stado-secret`. An SSH host key stored as a
        /// free-form secret loses the schema's guarantee that both halves are
        /// present, which is how one fleet key ended up shaped unlike its peers.
        #[arg(long = "type")]
        item_type: Option<String>,
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
    /// Recreate Weles internal authorities in the canonical owner vault from
    /// surviving owner credentials.
    #[command(name = "bootstrap-weles")]
    BootstrapWeles {
        /// Emit only the recreated item names as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Merge the retired Weles-dedicated vault into the canonical owner vault.
    ///
    /// Copies only the ids the canonical vault does not already hold, reports
    /// one outcome per item, reads the side vault and never writes to it, and
    /// prints item names, field-level reasons and counts — never a value.
    #[command(name = "adopt-weles-vault")]
    AdoptWelesVault {
        /// Emit the per-item outcomes as JSON instead of a table.
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
    /// Test unlock phrases found in transcripts against a local or remote
    /// vault, reporting which source name worked. Never prints a phrase.
    TryUnlock {
        /// Registry host holding the protected vault. Omit for the local vault.
        #[arg(long)]
        host: Option<String>,
        /// Test only the macOS Keychain entry, without replaying transcript phrases.
        #[arg(long, requires = "host")]
        keychain_only: bool,
    },
}

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    match command {
        // `doctor` is answered before any client exists. Every other verb needs
        // a grant, a token and a live service, which is exactly the set of
        // things this verb is for when one of them is what broke.
        SecretsCommands::Doctor { json } => doctor(json),
        SecretsCommands::InspectVault { vault, json } => inspect_vault(&vault, json),
        SecretsCommands::BootstrapWeles { json } => bootstrap_weles(json),
        SecretsCommands::AdoptWelesVault { json } => adopt_weles_vault(json),
        // Same reasoning as `doctor`: the transcripts are readable when the
        // vault is not, which is the only reason this verb is worth having.
        SecretsCommands::Harvest { json, restore, all } => {
            harvest(json, restore.as_deref(), all).await
        }
        // Also answered without a client: a protected key that nothing can
        // unlock is precisely the state where every other verb is unavailable.
        SecretsCommands::TryUnlock { host, keychain_only } => {
            try_unlock(host.as_deref(), keychain_only).await
        }
        SecretsCommands::Migrate { to } => migrate(to.as_deref()).await,
        SecretsCommands::Put { name, item_type } => {
            put(&client()?, &name, item_type.as_deref()).await
        }
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
        let selector = crate::credential_store::configured_selector()
            .map_err(|error| CmdError::click(error.to_string()))?;
        if selector.starts_with("skarbiec") {
            // Skarbiec can encrypt with public recipients even when no owner
            // here can decrypt. Refuse to bury the recovered value in that
            // state; other backends enforce their own write preconditions.
            let report = key_doctor_report(&skarbiec_binary()?)?;
            match report.get("status").and_then(Value::as_str) {
                Some("readable") | Some("empty") => {}
                _ => {
                    return Err(CmdError::click(format!(
                        "refusing to restore {name}: Skarbiec cannot be opened by any key here; own a readable vault first (see `stado secrets doctor`)"
                    )))
                }
            }
        }
        // One write path for every backend: a Skarbiec selector reaches the
        // vault through its owner inside the credential store, so this no
        // longer needs its own copy of that call.
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

/// The installed Skarbiec, resolved where every owner-path write resolves it.
fn skarbiec_binary() -> Result<std::path::PathBuf, CmdError> {
    crate::credential_store::owner::binary().map_err(|error| CmdError::click(error.to_string()))
}

/// The one vault an owner write lands in, resolved exactly where
/// [`crate::credential_store::owner`] resolves it.
///
/// This host runs one credential store. A verb that resolved its own path is
/// free to disagree with every other write in the process, and that is how a
/// vault dedicated to a single writer came to sit beside the canonical one
/// holding the only copy of Weles's credentials. Resolution that finds no
/// existing vault file is an error here rather than an invitation to create
/// one: a second vault created quietly is the defect, not the recovery.
fn owner_vault() -> Result<std::path::PathBuf, CmdError> {
    crate::credential_store::owner::vault().map_err(|error| CmdError::click(error.to_string()))
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

/// Nonsecret item metadata from one local vault file.
///
/// Read through the launcher because the launcher holds the unlock, and with
/// `--all` on purpose: a trashed id still occupies its name, so an inventory
/// that hid the trash would let a merge call an occupied id absent and write
/// over it.
fn vault_items(
    launcher: &std::path::Path,
    vault: &std::path::Path,
) -> Result<Vec<Value>, CmdError> {
    let output = std::process::Command::new(launcher)
        .arg("list")
        .arg("--all")
        .env("SKARBIEC_VAULT_FILE", vault)
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect {}: {}",
            launcher.display(),
            vault.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| CmdError::click("Skarbiec inventory was not a JSON array"))
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
    let items = vault_items(&launcher, std::path::Path::new(path))?;
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

/// Recreate Weles's internal authorities in the canonical owner vault.
///
/// These twelve items are authorities Weles issues to itself, so for a while
/// this command kept them in a vault of their own — it created
/// `weles-skarbiec.vault.json` under a `weles-skarbiec-owner` identity when the
/// path was missing. That made Weles the one writer in the fleet whose
/// credentials no other reader could open: `stado secrets ls`, the desktop
/// console and every consumer grant resolve against the canonical store, and an
/// item written into the side vault is absent from all of them.
///
/// The vault is now resolved, never created. A machine that holds no canonical
/// vault cannot recreate an authority, and saying so is the point: initializing
/// a fresh vault here would report twelve successful writes into a store that
/// nothing else on the host reads.
fn bootstrap_weles(json_output: bool) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    let vault = owner_vault()?;
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
        crate::credential_store::owner::store_json(
            &binary,
            &vault,
            item,
            "internal-authority",
            value,
            &json!({}),
        )
        .map_err(|error| CmdError::click(error.to_string()))?;
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
            "recreated {} Weles internal authority item(s) in {}",
            stored.len(),
            vault.display()
        );
    }
    Ok(())
}

/// The retired Weles-dedicated vault, named here only so its contents can be
/// moved into the canonical store and its file left behind.
const WELES_SIDE_VAULT: &str = "$HOME/.stado/weles-skarbiec.vault.json";

/// Tag Skarbiec reserves for authenticated Weles writes.
///
/// An owner copy cannot set it — `set-json` refuses the tag outright — so an
/// item carrying it would arrive in the canonical vault stripped of the one
/// marker that says which writer maintains it. That item is reported, not
/// copied.
const MANAGED_BY_WELES: &str = "managed:weles";

/// What happened to one id offered by the side vault.
///
/// `Failed` is reported to the operator as a skip like any other, because from
/// the vault's side nothing happened either way. It is kept apart from
/// `Skipped` only so a refused write reaches `$?`: a copy this command tried
/// and could not complete is an error, while a copy it declined to attempt is a
/// finding.
enum Adoption {
    Copied,
    AlreadyPresent,
    Skipped(String),
    Failed(String),
}

/// One item's canonical payload, read through the launcher that holds the
/// unlock.
///
/// The payload carries the value, so it is handed straight to the writer and
/// never rendered. A failure returns Skarbiec's own last line, which names the
/// item and its envelope and no more than that.
fn vault_payload(
    launcher: &std::path::Path,
    vault: &std::path::Path,
    item: &str,
) -> Result<Value, String> {
    let output = std::process::Command::new(launcher)
        .arg("get")
        .arg(item)
        .env("SKARBIEC_VAULT_FILE", vault)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("unreadable")
            .trim()
            .to_string());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "payload is not JSON".to_string())
}

/// Copy one id the canonical vault does not hold, or say why it was left alone.
///
/// Everything the owner path cannot carry across is refused rather than
/// half-copied. Tags are the reason there is anything to refuse: consumers
/// enumerate by tag, an owner write into an absent id creates it with no tags,
/// and a credential that arrives without its markers serves traffic while being
/// invisible to every reader that looks for it. Same for `extensions`, which
/// `store_json` does not carry.
fn adopt_one(
    binary: &std::path::Path,
    launcher: &std::path::Path,
    side: &std::path::Path,
    canonical: &std::path::Path,
    id: &str,
    metadata: &Value,
) -> Adoption {
    if metadata
        .get("deleted")
        .and_then(Value::as_bool)
        .unwrap_or_default()
    {
        return Adoption::Skipped("trashed in the side vault".to_string());
    }
    let tags: Vec<&str> = metadata
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| tags.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if tags.contains(&MANAGED_BY_WELES) {
        return Adoption::Skipped(format!(
            "carries {MANAGED_BY_WELES}, which only an authenticated Weles write can set; acquire it again through Weles"
        ));
    }
    if !tags.is_empty() {
        return Adoption::Skipped(format!(
            "carries tag(s) {} that an owner copy cannot preserve",
            tags.join(",")
        ));
    }
    let payload = match vault_payload(launcher, side, id) {
        Ok(payload) => payload,
        Err(reason) => return Adoption::Skipped(reason),
    };
    if payload.get("extensions").is_some() {
        return Adoption::Skipped(
            "payload carries extensions that an owner copy cannot preserve".to_string(),
        );
    }
    let Some(kind) = payload.get("kind").and_then(Value::as_str) else {
        return Adoption::Skipped("payload declares no kind".to_string());
    };
    let Some(fields) = payload.get("fields") else {
        return Adoption::Skipped("payload carries no fields".to_string());
    };
    let context = payload.get("context").cloned().unwrap_or_else(|| json!({}));
    match crate::credential_store::owner::store_json(binary, canonical, id, kind, fields, &context)
    {
        Ok(()) => Adoption::Copied,
        Err(error) => Adoption::Failed(error.to_string()),
    }
}

/// Merge the retired Weles-dedicated vault into the canonical owner vault.
///
/// One direction, and only into free names. An id the canonical vault already
/// holds is reported and left exactly as it is: the canonical copy is the one
/// Weles's own authenticated writes have been maintaining, so overwriting it
/// from a file last touched during the split would replace a current credential
/// with an older one. That is why the collision case is a report rather than a
/// merge policy.
///
/// The side vault is opened read-only and never written, not even to trash what
/// was copied. Removing the file is the operator's call once this report shows
/// nothing left to adopt, and a command that deleted its own evidence would
/// leave no way to check that claim.
fn adopt_weles_vault(json_output: bool) -> Result<(), CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let side = std::path::PathBuf::from(WELES_SIDE_VAULT.replace("$HOME", &home));
    let canonical = owner_vault()?;
    if !side.is_file() {
        return Err(CmdError::click(format!(
            "no Weles vault at {}; nothing to adopt, and {} is already the only credential store on this host",
            side.display(),
            canonical.display()
        )));
    }
    if canonical == side {
        return Err(CmdError::click(format!(
            "{} is the resolved owner vault; adoption needs a side vault and a canonical vault, not one file twice",
            side.display()
        )));
    }
    let binary = skarbiec_binary()?;
    let launcher = skarbiec_launcher()?;
    let present: std::collections::BTreeSet<String> = vault_items(&launcher, &canonical)?
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut adoptions = Vec::new();
    for item in vault_items(&launcher, &side)? {
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return Err(CmdError::click(format!(
                "{} listed an item without an id",
                side.display()
            )));
        };
        let adoption = if present.contains(id) {
            Adoption::AlreadyPresent
        } else {
            adopt_one(&binary, &launcher, &side, &canonical, id, &item)
        };
        adoptions.push((id.to_string(), adoption));
    }
    let count = |wanted: fn(&Adoption) -> bool| {
        adoptions
            .iter()
            .filter(|(_, adoption)| wanted(adoption))
            .count()
    };
    let copied = count(|adoption| matches!(adoption, Adoption::Copied));
    let already = count(|adoption| matches!(adoption, Adoption::AlreadyPresent));
    let failed = count(|adoption| matches!(adoption, Adoption::Failed(_)));
    let skipped = count(|adoption| matches!(adoption, Adoption::Skipped(_))) + failed;
    let outcome = |adoption: &Adoption| match adoption {
        Adoption::Copied => "copied",
        Adoption::AlreadyPresent => "already-present",
        Adoption::Skipped(_) | Adoption::Failed(_) => "skipped",
    };
    let reason = |adoption: &Adoption| match adoption {
        Adoption::Copied | Adoption::AlreadyPresent => None,
        Adoption::Skipped(reason) => Some(reason.clone()),
        Adoption::Failed(reason) => Some(format!("write refused: {reason}")),
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "side_vault": side,
                "vault": canonical,
                "copied": copied,
                "already_present": already,
                "skipped": skipped,
                "items": adoptions
                    .iter()
                    .map(|(id, adoption)| json!({
                        "item": id,
                        "outcome": outcome(adoption),
                        "reason": reason(adoption),
                    }))
                    .collect::<Vec<Value>>(),
            }))?
        );
    } else {
        let rows = adoptions
            .iter()
            .map(|(id, adoption)| {
                vec![
                    id.clone(),
                    outcome(adoption).to_string(),
                    reason(adoption).unwrap_or_else(unknown),
                ]
            })
            .collect::<Vec<Vec<String>>>();
        table::print(&["ITEM", "OUTCOME", "REASON"], &rows);
        println!(
            "{copied} copied, {already} already present, {skipped} skipped: {} -> {}",
            side.display(),
            canonical.display()
        );
    }
    if failed != usize::default() {
        return Err(CmdError::click(format!(
            "{failed} item(s) could not be written into {}",
            canonical.display()
        )));
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
async fn try_unlock(host: Option<&str>, keychain_only: bool) -> Result<(), CmdError> {
    let candidates = if keychain_only {
        Vec::new()
    } else {
        crate::transcripts::unlock_candidates()
    };
    if candidates.is_empty() && !keychain_only {
        return Err(CmdError::click(
            "no unlock phrase of any kind survives in transcript runtime output",
        ));
    }
    match host {
        Some(host) => try_unlock_remote(host, &candidates, keychain_only).await,
        None => try_unlock_local(&candidates),
    }
}

fn try_unlock_local(candidates: &[(String, String)]) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    println!(
        "testing {} distinct phrase(s) from transcript history",
        candidates.len()
    );
    for (name, phrase) in candidates {
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

async fn try_unlock_remote(
    host: &str,
    candidates: &[(String, String)],
    keychain_only: bool,
) -> Result<(), CmdError> {
    use base64::Engine as _;

    let target = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut encoded = String::new();
    for (name, phrase) in candidates {
        encoded.push_str(&base64::engine::general_purpose::STANDARD.encode(name.as_bytes()));
        encoded.push(' ');
        encoded.push_str(&base64::engine::general_purpose::STANDARD.encode(phrase.as_bytes()));
        encoded.push('\n');
    }
    let keychain_source = base64::engine::general_purpose::STANDARD
        .encode(b"macOS Keychain service skarbiec-vault");
    let failure = if keychain_only {
        "the host keychain entry does not open the remote vault"
    } else {
        "neither the host keychain nor any surviving transcript phrase opens the remote vault"
    };
    let script = format!(
        r#"set -euo pipefail
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
binary="$HOME/.stado/bin/skarbiec"
vault="$HOME/.stado/skarbiec.vault.json"
unlock="$HOME/.stado/skarbiec-unlock"
try_phrase() {{
  source_b64="$1"
  phrase="$2"
  set +e
  report="$(GNUPGHOME="$HOME/.gnupg" SKARBIEC_VAULT_FILE="$vault" SKARBIEC_UNLOCK="$phrase" "$binary" key-doctor 2>/dev/null)"
  status=$?
  set -e
  if [ "$status" -eq 0 ] && printf '%s' "$report" | /usr/bin/grep -q '"status"[[:space:]]*:[[:space:]]*"readable"'; then
    umask 077
    printf '%s' "$phrase" > "$unlock.new"
    /bin/chmod 600 "$unlock.new"
    /bin/mv -f "$unlock.new" "$unlock"
    printf 'STADO_UNLOCK\t%s\n' "$source_b64"
    exit 0
  fi
}}
while IFS= read -r candidate; do
  [ "$candidate" != "$vault" ] || continue
  set +e
  report="$(GNUPGHOME="$HOME/.gnupg" SKARBIEC_VAULT_FILE="$candidate" "$binary" key-doctor 2>/dev/null)"
  status=$?
  set -e
  if [ "$status" -eq 0 ] && printf '%s' "$report" | /usr/bin/grep -q '"status"[[:space:]]*:[[:space:]]*"readable"'; then
    stamp="$(/bin/date -u +%Y%m%dT%H%M%SZ)"
    /bin/cp -p "$vault" "$vault.unreadable-$stamp"
    /bin/cp -p "$candidate" "$vault.new"
    /bin/chmod 600 "$vault.new"
    /bin/mv -f "$vault.new" "$vault"
    printf 'STADO_BACKUP\t%s\n' "$candidate"
    exit 0
  fi
done < <(/usr/bin/find "$HOME/.stado" -maxdepth 4 -type f \( -name '*skarbiec*vault*.json*' -o -name '*skarbiec*.bak' \) -print)
if [ "$(/usr/bin/uname -s)" = Darwin ]; then
  keychain_phrase="$(/bin/launchctl asuser "$(/usr/bin/id -u)" /usr/bin/security find-generic-password -s skarbiec-vault -w 2>/dev/null || true)"
  if [ -z "$keychain_phrase" ]; then
    keychain_phrase="$(/usr/bin/security find-generic-password -s skarbiec-vault -w 2>/dev/null || true)"
  fi
  if [ -n "$keychain_phrase" ]; then
    try_phrase "{keychain_source}" "$keychain_phrase"
  fi
fi
while IFS=' ' read -r source_b64 phrase_b64; do
  [ -n "$source_b64" ] || continue
  phrase="$(printf '%s' "$phrase_b64" | /usr/bin/base64 "$decode")"
  try_phrase "$source_b64" "$phrase"
done <<'STADO_UNLOCK_CANDIDATES'
{encoded}STADO_UNLOCK_CANDIDATES
printf '%s\n' '{failure}' >&2
exit 2
"#
    );
    if keychain_only {
        println!("testing the host keychain against the vault on {host}");
    } else {
        println!(
            "testing the host keychain and {} distinct transcript phrase(s) against the vault on {host}",
            candidates.len()
        );
    }
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &target,
        &script,
        if keychain_only {
            std::time::Duration::from_secs(30)
        } else {
            std::time::Duration::from_secs(900)
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(
            crate::deploy::host_channel::last_error_line(
                &output,
                "remote vault unlock recovery failed",
            ),
        ));
    }
    if let Some(path) = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_BACKUP\t"))
    {
        println!("restored the readable vault backup {path} on {host}");
        println!("preserved the unreadable vault beside it with a UTC suffix");
        return Ok(());
    }
    let encoded_name = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNLOCK\t"))
        .ok_or_else(|| CmdError::click("remote unlock recovery returned no source marker"))?;
    let name = base64::engine::general_purpose::STANDARD
        .decode(encoded_name)
        .map_err(|error| CmdError::click(format!("remote unlock source is invalid: {error}")))?;
    println!(
        "the vault on {host} OPENS with the phrase recorded under {}",
        String::from_utf8_lossy(&name)
    );
    println!("stored it in the host's owner-only persistent unlock file");
    Ok(())
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

async fn put(
    vault: &crate::skarbiec::Client,
    name: &str,
    item_type: Option<&str>,
) -> Result<(), CmdError> {
    let input = read_value_from_stdin()?;
    if input.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the value in (stado secrets put NAME < file)",
        ));
    }
    let value: Value = serde_json::from_str(&input).unwrap_or_else(|_| json!({"value": input}));
    // The kind is the payload's shape, so the payload decides it when it says
    // so. Forcing `stado-secret` on every write is how one item ends up holding
    // a key pair with no schema requiring its public half.
    let declared = value
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty());
    let item_kind = item_type.or(declared).unwrap_or("stado-secret");
    vault
        .write_item(name, item_kind, &value)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("stored credential item {name:?} as {item_kind:?}");
    Ok(())
}

async fn get(
    vault: &crate::skarbiec::Client,
    name: &str,
    field: Option<&str>,
) -> Result<(), CmdError> {
    if let Some(field) = field {
        let raw = vault
            .read_string(name, field)
            .await
            .map_err(|err| CmdError::click(err.to_string()))?
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "credential item {name:?} has no non-empty string field {field:?}"
                ))
            })?;
        println!("{raw}");
        return Ok(());
    }
    let value = vault.read_item(name).await.map_err(|err| {
        CmdError::click(format!(
            "{err}; this store answers per field: name one with --field"
        ))
    })?;
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

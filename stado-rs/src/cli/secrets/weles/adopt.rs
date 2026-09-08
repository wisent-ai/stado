//! Merging the retired Weles-dedicated vault into the canonical owner vault:
//! one direction, only into free names, and no value ever rendered.

use serde_json::{json, Value};

use crate::cli::{table, CmdError};

use crate::cli::secrets::store::inventory::vault_items;
use crate::cli::secrets::store::resolve::{
    owner_vault, skarbiec_binary, skarbiec_launcher, unknown,
};

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
pub(crate) fn adopt_weles_vault(json_output: bool) -> Result<(), CmdError> {
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

//! Host-key custody in Skarbiec: `key add|ls|rm|install|check`, with
//! generation and rotation in [`rotate`].
//!
//! Private host keys live as Skarbiec items — never on disk under the
//! user's home, never printed. Listing shows metadata only. Using a key
//! means materializing it into a transient owner-only file for the single
//! remote call and removing it right after. Where keys live is a fleet
//! decision: `registry.enrollment.key_custody` is `skarbiec` (vault
//! first) or `openssh` (agent, config, default key files), read from the
//! central catalog on every channel.

pub mod rotate;

use serde_json::{json, Value};
use stado::deploy::bootstrap::ssh_argv;
use stado::deploy::{CommandSpec, Runner};
use stado::skarbiec::Client;

/// Vault item id prefix for host keys; the target name follows it.
const ITEM_PREFIX: &str = "stado-ssh-";
const ITEM_TYPE: &str = "ssh-key";

/// Vault item id for one target's host key. Pure.
pub fn item_id(target: &str) -> String {
    format!("{ITEM_PREFIX}{target}")
}

pub fn authorized_keys_line(public_key: &str, comment: &str) -> String {
    format!("{public_key} {comment}")
}

pub(crate) async fn run_checked(
    runner: &Runner,
    spec: CommandSpec,
    what: &str,
) -> Result<String, String> {
    let output = runner(spec).await?;
    if output.ok() {
        Ok(output.stdout)
    } else {
        Err(format!("{what} failed: {}", output.detail()))
    }
}

/// Key management is an operator action: it uses the operator consumer
/// (write access to vault items), not the scoped runtime consumers.
pub(crate) async fn configured_client() -> Result<Client, String> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME is not set; cannot locate the operator grant".to_string())?;
    let token_file = format!(
        "{}/.stado/local-operator-skarbiec-token",
        home.to_string_lossy()
    );
    Client::new(stado::config::skarbiec_url(), "local-operator", &token_file)
        .map_err(|exc| exc.to_string())
}

/// Write the private key to a transient owner-only file; the caller removes it.
async fn materialize(
    runner: &Runner,
    client: &Client,
    target: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let item = match client.read_item(&item_id(target)).await {
        Ok(item) => item,
        Err(_) => return Ok(None),
    };
    let private_key = item
        .get("private_key")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("vault item {} has no private_key field", item_id(target)))?;
    let path = std::env::temp_dir().join(format!("stado-fleet-key-{}", std::process::id()));
    std::fs::write(&path, format!("{private_key}\n")).map_err(|exc| exc.to_string())?;
    run_checked(
        runner,
        CommandSpec::new(vec![
            "chmod".to_string(),
            "600".to_string(),
            path.to_string_lossy().to_string(),
        ]),
        "chmod of the materialized key",
    )
    .await?;
    Ok(Some(path))
}

/// Build the channel argv honoring the fleet's custody declaration:
/// `skarbiec` uses the target's vault key with `-i` when stored,
/// `openssh` always uses the OpenSSH default resolution.
pub async fn channel_argv(
    runner: &Runner,
    target: &str,
    destination: &str,
    command: &str,
) -> Result<(Vec<String>, Option<std::path::PathBuf>), String> {
    let document = stado::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let custody = crate::enroll::catalog::parse_enrollment(&document)?.key_custody;
    let materialized = if custody == "openssh" {
        None
    } else {
        let client = configured_client().await?;
        materialize(runner, &client, target).await?
    };
    let argv = match &materialized {
        Some(path) => vec![
            "ssh".to_string(),
            "-i".to_string(),
            path.to_string_lossy().to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            destination.to_string(),
            command.to_string(),
        ],
        None => ssh_argv(destination, command),
    };
    Ok((argv, materialized))
}

/// `key add TARGET --from PATH` — import an existing private key into the
/// vault. The private material is never printed; only the fingerprint is.
pub async fn add(runner: &Runner, target: &str, from: &str) -> Result<bool, String> {
    let private_key = std::fs::read_to_string(from)
        .map_err(|exc| format!("cannot read key file {from}: {exc}"))?;
    let public_key = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -y",
    )
    .await?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-lf".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -lf",
    )
    .await?;
    let fingerprint = fingerprint_line
        .split_whitespace()
        .find(|part| part.starts_with("SHA256:"))
        .unwrap_or_default()
        .to_string();
    let key_type = fingerprint_line
        .rsplit('(')
        .next()
        .map(|part| part.trim().trim_end_matches(')').to_string())
        .unwrap_or_default();
    let client = configured_client().await?;
    client
        .write_item(
            &item_id(target),
            ITEM_TYPE,
            &json!({
                "private_key": private_key.trim(),
                "public_key": public_key.trim(),
                "key_type": key_type,
                "fingerprint": fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .map_err(|exc| exc.to_string())?;
    println!("stored vault item {} ({})", item_id(target), fingerprint);
    Ok(true)
}

/// `key ls` — metadata of every vault host key. No private fields.
pub async fn ls() -> Result<bool, String> {
    let client = configured_client().await?;
    let items = client.list_items().await.map_err(|exc| exc.to_string())?;
    let mut shown = Vec::new();
    for item in items {
        if !item.id.starts_with(ITEM_PREFIX) {
            continue;
        }
        let document = client.read_item(&item.id).await.map_err(|exc| exc.to_string())?;
        let fingerprint = document
            .get("fingerprint")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let key_type = document
            .get("key_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        shown.push(format!("{}\t{}\t{}", item.id, key_type, fingerprint));
    }
    if shown.is_empty() {
        println!("no host keys in the vault");
    } else {
        for line in &shown {
            println!("{line}");
        }
    }
    Ok(true)
}

/// `key rm TARGET` — delete the target's vault host key.
pub async fn rm(target: &str) -> Result<bool, String> {
    let client = configured_client().await?;
    client
        .delete_item(&item_id(target))
        .await
        .map_err(|exc| exc.to_string())?;
    println!("removed vault item {}", item_id(target));
    Ok(true)
}

/// `key install TARGET` — append the vault public key to the target's
/// authorized_keys, through the channel that already reaches the host.
pub async fn install(runner: &Runner, target: &str) -> Result<bool, String> {
    let client = configured_client().await?;
    let document = client
        .read_item(&item_id(target))
        .await
        .map_err(|exc| exc.to_string())?;
    let public_key = document
        .get("public_key")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("vault item {} has no public_key field", item_id(target)))?;
    let registry = stado::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    let target_entry = registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?;
    let destination = target_entry
        .ssh
        .as_deref()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))?;
    let line = authorized_keys_line(public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, materialized) = channel_argv(runner, target, destination, &command).await?;
    let result = run_checked(runner, CommandSpec::new(argv), "authorized_keys install").await;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    result?;
    println!("installed public key for '{target}' into authorized_keys on {destination}");
    Ok(true)
}

/// `key check TARGET` — verify the vault key actually opens the channel:
/// materialize, run `hostname` remotely, remove the materialized file.
pub async fn check(runner: &Runner, target: &str) -> Result<bool, String> {
    let registry = stado::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    let target_entry = registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?;
    let destination = target_entry
        .ssh
        .as_deref()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))?;
    let (argv, materialized) = channel_argv(runner, target, destination, "hostname").await?;
    let using_vault = materialized.is_some();
    let result = run_checked(runner, CommandSpec::new(argv), "hostname over the channel").await;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    let answered = result?;
    if using_vault {
        println!("vault key verified: {destination} answered as {}", answered.trim());
    } else {
        println!(
            "no vault key for '{target}'; {destination} answered as {} via the OpenSSH default resolution",
            answered.trim()
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_id_scopes_by_target() {
        assert_eq!(item_id("mini"), "stado-ssh-mini");
    }

    #[test]
    fn authorized_keys_line_carries_comment() {
        assert_eq!(
            authorized_keys_line("ssh-ed25519 AAAA", "stado-ssh-mini"),
            "ssh-ed25519 AAAA stado-ssh-mini"
        );
    }
}

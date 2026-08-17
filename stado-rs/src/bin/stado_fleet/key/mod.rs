//! SSH host keys in the globally selected credential store:
//! `key add|ls|rm|install|check`, plus generation and rotation in [`rotate`].
//!
//! Private material is never printed. A remote call reads the target key from
//! the selected store, writes one owner-only transient file for `ssh -i`, then
//! removes it. There is no OpenSSH home-directory fallback: changing
//! `STADO_CREDENTIALS_STORE` is a credential migration, not a second lookup
//! path.

mod channel;
pub use channel::channel_argv;
pub mod rotate;

use serde_json::json;
use stado::deploy::{CommandSpec, Runner};
use stado::skarbiec::Client;

/// Credential item id prefix for host keys; the target name follows it.
const ITEM_PREFIX: &str = "stado-ssh-";
/// Skarbiec's canonical kind for a private/public pair. `ssh-key` is an input
/// spelling, not a kind: the vault stores `private_key` and `public_key` as the
/// pair's fields and keeps the fingerprint and key type as context, and it
/// refuses a payload that claims any other kind.
const ITEM_TYPE: &str = "key-pair";

/// Credential item id for one target's host key.
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

/// Key management is an operator action routed through the globally selected
/// credential store; Skarbiec uses the external admin bootstrap grant.
pub(crate) fn configured_client() -> Result<Client, String> {
    let credentials =
        stado::credential_store::admin_credentials().map_err(|exc| exc.to_string())?;
    Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|exc| exc.to_string())
}

/// `key add TARGET --from PATH` — move an existing private key into the
/// selected store. The source file is removed only after a read-back verifies
/// the stored material; private content is never printed.
pub async fn add(runner: &Runner, target: &str, from: &str) -> Result<bool, String> {
    let metadata = std::fs::symlink_metadata(from)
        .map_err(|exc| format!("cannot inspect key file {from}: {exc}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!(
            "key source {from} must be a regular file, not a symlink or special file"
        ));
    }
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
    let id = item_id(target);
    let client = configured_client()?;
    client
        .write_described(
            &id,
            ITEM_TYPE,
            &json!({
                "private_key": private_key.trim(),
                "public_key": public_key.trim(),
            }),
            &json!({
                "key_type": key_type,
                "fingerprint": fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .map_err(|exc| exc.to_string())?;
    // Read back the three fields by name: a broker that requires a named
    // field refuses the whole-item form, and this verification is the only
    // thing standing between a half-written key and a deleted source file.
    let read_back = |field: &'static str| {
        let client = &client;
        let id = id.clone();
        async move { client.read_string(&id, field).await.ok().flatten() }
    };
    let verified = read_back("private_key").await.as_deref() == Some(private_key.trim())
        && read_back("public_key").await.as_deref() == Some(public_key.trim())
        && read_back("fingerprint").await.as_deref() == Some(fingerprint.as_str());
    if !verified {
        let _ = client.delete_item(&id).await;
        return Err(format!(
            "credential item {id} failed read-back verification; the source file was preserved"
        ));
    }
    if let Err(error) = std::fs::remove_file(from) {
        let rollback = client.delete_item(&id).await;
        return Err(match rollback {
            Ok(()) => format!(
                "cannot remove source key {from}: {error}; the credential-store write was rolled back"
            ),
            Err(rollback_error) => format!(
                "cannot remove source key {from}: {error}; store rollback also failed: {rollback_error}"
            ),
        });
    }
    let _ = std::fs::remove_file(format!("{from}.pub"));
    println!("moved key into credential item {id} ({fingerprint})");
    Ok(true)
}

/// `key ls` — metadata of every stored SSH host key. No private fields.
pub async fn ls() -> Result<bool, String> {
    let client = configured_client()?;
    let items = client.list_items().await.map_err(|exc| exc.to_string())?;
    let mut shown = Vec::new();
    for item in items {
        if !item.id.starts_with(ITEM_PREFIX) {
            continue;
        }
        // `fingerprint` and `key_type` are schema CONTEXT on a `key-pair`, not
        // fields: Skarbiec's canonical form keeps the two halves of the key as
        // fields and everything descriptive beside them. Asking for them as
        // fields is refused, and the refusal used to arrive here as two blank
        // columns, which reads as a key with no fingerprint rather than as a
        // read of the wrong place. The private field is never asked for.
        let context = client
            .read_field(&item.id, "context")
            .await
            .unwrap_or_else(|_| json!({}));
        let described = |name: &str| {
            context
                .get(name)
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        shown.push(format!(
            "{}\t{}\t{}",
            item.id,
            described("key_type"),
            described("fingerprint")
        ));
    }
    if shown.is_empty() {
        println!("no SSH host keys in the credential store");
    } else {
        for line in &shown {
            println!("{line}");
        }
    }
    Ok(true)
}

/// `key rm TARGET` — delete the target's SSH host key.
pub async fn rm(target: &str) -> Result<bool, String> {
    let client = configured_client()?;
    client
        .delete_item(&item_id(target))
        .await
        .map_err(|exc| exc.to_string())?;
    println!("removed credential item {}", item_id(target));
    Ok(true)
}

/// `key install TARGET` — append the stored public key to the target's
/// authorized_keys through the existing credential-store-backed channel.
pub async fn install(runner: &Runner, target: &str) -> Result<bool, String> {
    let client = configured_client()?;
    let public_key = client
        .read_string(&item_id(target), "public_key")
        .await
        .map_err(|exc| exc.to_string())?
        .ok_or_else(|| {
            format!(
                "credential item {} has no public_key field",
                item_id(target)
            )
        })?;
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
    let line = authorized_keys_line(&public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, destination, &command).await?;
    run_checked(runner, CommandSpec::new(argv), "authorized_keys install").await?;
    println!("installed public key for '{target}' into authorized_keys on {destination}");
    Ok(true)
}

/// `key check TARGET` — verify the selected-store key opens the channel.
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
    let (argv, _key) = channel_argv(target, destination, "hostname").await?;
    let answered = run_checked(runner, CommandSpec::new(argv), "hostname over the channel").await?;
    println!(
        "credential-store key verified: {destination} answered as {}",
        answered.trim()
    );
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

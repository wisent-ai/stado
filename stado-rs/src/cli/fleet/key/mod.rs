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
use crate::deploy::{CommandSpec, Runner};
use crate::skarbiec::Client;

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
        crate::credential_store::admin_credentials().map_err(|exc| exc.to_string())?;
    Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|exc| exc.to_string())
}

/// Fields of a key-pair item the SSH channel's reader must be able to read.
/// Grants are per item, so these are exactly the capabilities a freshly minted
/// key is missing.
const CHANNEL_FIELDS: [&str; 2] = ["private_key", "public_key"];

/// Finish a key write: make the item readable by the consumer the SSH channel
/// reads it through, then prove it through that same consumer.
///
/// Two distinct stores are in play. An owner write reaches a vault FILE; the
/// channel reaches a BROKER, authenticating as the administrative consumer of
/// [`crate::credential_store::admin_credentials`]. Skarbiec authorizes reads per
/// item, so the write leaves the item invisible to that consumer until its grant
/// is widened — which is why every freshly minted key used to be dead on
/// arrival. And on a host whose broker forwards to another machine's vault, the
/// two stores are not the same store at all, so a key that looks written is
/// invisible to the fleet. Neither condition is detectable later from anywhere
/// nearer than the failing host, so the write is not finished until the reader
/// can see what was written.
///
/// `verify` names the fields read back and the values they must carry. Values
/// are compared, never printed.
pub(crate) async fn settle_readable(
    client: &Client,
    id: &str,
    verify: &[(&str, &str)],
) -> Result<(), String> {
    // A file store answers its owner directly and has no grants to widen; the
    // read-back there goes through the store, not through a broker that may not
    // exist on that deployment.
    let brokered = crate::credential_store::skarbiec_url().is_some();
    if brokered {
        let credentials =
            crate::credential_store::admin_credentials().map_err(|exc| exc.to_string())?;
        let outcome = crate::credential_store::grant::grant_field_reads(
            &credentials.consumer,
            std::path::Path::new(&credentials.token_file),
            id,
            &CHANNEL_FIELDS,
        )
        .map_err(|exc| {
            format!("cannot make {id} readable by {}: {exc}", credentials.consumer)
        })?;
        if outcome.wrote() {
            println!(
                "granted {} read on {} ({} capabilities held, was {})",
                credentials.consumer,
                outcome.added.join(", "),
                outcome.held_after,
                outcome.held_before
            );
        }
    }
    for (field, expected) in verify {
        let read = if brokered {
            client
                .read_field(id, field)
                .await
                .map(|value| value.as_str().map(str::to_string))
        } else {
            client.read_string(id, field).await
        };
        // Every way this can end badly says the same thing. The item was
        // written and its fields were granted a moment ago, so a reader that
        // refuses them, or answers with something else, is not reading the
        // vault this write reached — nothing the caller can fix by retrying or
        // by granting more.
        let reason = match read {
            Ok(stored) if stored.as_deref().map(str::trim) == Some(expected.trim()) => continue,
            Ok(Some(_)) => "a different value".to_string(),
            Ok(None) => "nothing".to_string(),
            Err(crate::skarbiec::SkarbiecError::Response { status, detail })
                if status == reqwest::StatusCode::FORBIDDEN.as_u16()
                    || status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
            {
                format!("HTTP {status}: {}", detail.trim())
            }
            Err(error) => return Err(error.to_string()),
        };
        return Err(format!(
            "wrote {id} and granted its fields, but the reader that opens the channel serves \
             {reason} for {field}. This machine's vault is not the one the fleet reads: mint on \
             the host that holds it (`stado host vaults` names them), or point \
             SKARBIEC_VAULT_FILE at that vault"
        ));
    }
    Ok(())
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
    // The source file is about to be deleted, so the read-back is the only
    // thing standing between a half-written key and a key that exists nowhere.
    // It reads the material by name through the consumer the channel uses:
    // `fingerprint` is schema context rather than a field, carries no grant,
    // and proves nothing about whether this key can open a connection.
    if let Err(error) = settle_readable(
        &client,
        &id,
        &[
            ("private_key", private_key.trim()),
            ("public_key", public_key.trim()),
        ],
    )
    .await
    {
        let _ = client.delete_item(&id).await;
        return Err(format!(
            "credential item {id} failed read-back verification: {error}. The source file was \
             preserved"
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
    let registry = crate::targets::load_registry_auto()
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
    let registry = crate::targets::load_registry_auto()
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

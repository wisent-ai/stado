//! Item naming, the configured client, and the write that is not finished
//! until the channel's reader can see what was written.

use crate::deploy::{CommandSpec, Runner};
use crate::skarbiec::Client;

/// Credential item id prefix for host keys; the target name follows it.
pub(in crate::cli::fleet::key) const ITEM_PREFIX: &str = "stado-ssh-";
/// Skarbiec's canonical kind for a private/public pair. `ssh-key` is an input
/// spelling, not a kind: the vault stores `private_key` and `public_key` as the
/// pair's fields and keeps the fingerprint and key type as context, and it
/// refuses a payload that claims any other kind.
pub(in crate::cli::fleet::key) const ITEM_TYPE: &str = "key-pair";

/// Credential item id for one target's host key.
pub fn item_id(target: &str) -> String {
    format!("{ITEM_PREFIX}{target}")
}

/// One `authorized_keys` line: the key's type and blob, then exactly one
/// comment.
///
/// The stored key still carries the comment `ssh-keygen -C` put on it, which is
/// already the credential item id, so pasting it verbatim in front of another
/// comment produced a line naming the same item twice — what `key install`
/// appends today. Only the first two fields of a public key are the key; the
/// rest is commentary, and this owns the commentary.
pub fn authorized_keys_line(public_key: &str, comment: &str) -> String {
    let mut fields = public_key.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob} {comment}"),
        // Not a two-field key: pass it through rather than silently truncating
        // something the caller will have to recognize in an error message.
        _ => format!("{} {comment}", public_key.trim()),
    }
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
        crate::skarbiec::GrantMode::RereadPerRequest,
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
            format!(
                "cannot make {id} readable by {}: {exc}",
                credentials.consumer
            )
        })?;
        if outcome.wrote() {
            // Progress, not output: `fleet invite --json` mints a channel key on
            // its way to printing one JSON document, and a widened grant is not
            // part of that document. stderr keeps the operator informed without
            // making every JSON consumer parse around it.
            eprintln!(
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
            Err(error)
                if error.status().is_some_and(|status| {
                    status == reqwest::StatusCode::FORBIDDEN.as_u16()
                        || status == reqwest::StatusCode::NOT_FOUND.as_u16()
                }) =>
            {
                error.to_string()
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

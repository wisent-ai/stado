use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::item::{
    read_vault_phase, read_vault_updated_at, VaultItemSummary, VAULT_FIELD_SUMMARY_PROGRAM,
};
use crate::cli::host::secrets::vault::vault_word;

/// `stado credentials item show --host TARGET ITEM` — what one item holds,
/// values.
///
/// `vault-item-put` had no counterpart, and the absence was not cosmetic: an
/// operator who had just written an item through the host channel could not
/// confirm from a workstation that the host held it, because
/// `retag-vault-item`'s read reports state, revision and tags and nothing
/// about the payload, `stado credentials get` reads the local store, and
/// `skarbiec get` is not a host-exec command. A migration wrote seven bundles
/// and twenty credential fields into a workstation vault that nothing on the
/// fleet reads, and the only reason it surfaced was a 401 from Brama.
///
/// So this reports the field NAMES with, per field, the value's length and its
/// SHA-256 — enough to compare against a local copy's digest and answer "does
/// the host hold what this row references", and never enough to learn the
/// value.
pub async fn vault_item_show(
    target: &str,
    item: &str,
    field: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    if let Some(field) = field {
        vault_word("field", field)?;
    }
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: {item} could not be read: {detail}",
            resolved.name
        ))
    };

    // The encrypted record first: an absent item is an answer, and it is the
    // answer that costs nothing to give.
    let record = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(refused)?;
    let updated_at = read_vault_updated_at(&resolved, &vault, item, &runner)
        .await
        .unwrap_or_else(|_| "-".to_string());
    if record.state == "absent" {
        return Err(CmdError::click(format!(
            "{} declares no credential item {item}; add it to the vault declared by secrets.skarbiec.vault_file",
            resolved.name
        )));
    }

    let summary_text = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} get {} --json | python3 -c {}",
            crate::deploy::shlex_quote(&gnupg_home),
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(VAULT_FIELD_SUMMARY_PROGRAM),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !summary_text.ok() {
        // The last line of a remote failure is often the least informative one
        // - a decryption failure ends in a backtrace note - so the refusal
        // carries the host's own words, trimmed to what fits a terminal.
        let detail = summary_text
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(4)
            .collect::<Vec<&str>>()
            .into_iter()
            .rev()
            .collect::<Vec<&str>>()
            .join("; ");
        return Err(refused(if detail.is_empty() {
            "the host could not summarise the item's fields".to_string()
        } else {
            detail
        }));
    }
    let summary: VaultItemSummary = serde_json::from_str(summary_text.stdout.trim())
        .map_err(|error| refused(format!("the host's field summary did not parse: {error}")))?;
    let mut fields = summary.fields;
    if let Some(wanted) = field {
        fields.retain(|entry| entry.name == wanted);
        if fields.is_empty() {
            return Err(refused(format!("the item holds no field {wanted}")));
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "state": record.state,
                "revision": record.revision,
                "tags": record.tags,
                "updated_at": updated_at,
                "kind": summary.kind,
                "schema": summary.schema,
                "fields": fields
                    .iter()
                    .map(|entry| json!({
                        "name": entry.name,
                        "length": entry.length,
                        "sha256": entry.sha256,
                        "text": entry.text,
                    }))
                    .collect::<Vec<Value>>(),
            }))?
        );
        return Ok(());
    }
    println!("host:       {}", resolved.name);
    println!("item:       {item}");
    println!("kind:       {}", summary.kind.as_deref().unwrap_or("-"));
    println!("schema:     {}", summary.schema.as_deref().unwrap_or("-"));
    println!("state:      {}", record.state);
    println!("revision:   {}", record.revision);
    println!("tags:       {}", record.tags);
    println!("updated_at: {updated_at}");
    for entry in &fields {
        println!(
            "field:      {} {} bytes sha256={}{}",
            entry.name,
            entry.length,
            entry.sha256,
            if entry.text { "" } else { " (structured)" }
        );
    }
    Ok(())
}

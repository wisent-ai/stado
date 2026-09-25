use serde_json::json;

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::item::read_vault_phase;
use crate::cli::host::secrets::vault::vault_word;

/// Give one Skarbiec item on TARGET a new id, keeping its payload, revision
/// history and tags, and report both ids before and after.
///
/// A product has one identity and one item named after itself; the fleet
/// still holds items named after roles (`<product>-release-publisher`). A copy
/// would leave two items with one secret, and a `set-json` under the new id
/// would need the plaintext in hand and start the history over, so the item
/// moves with Skarbiec's own `rename`, on the host whose owner key can write
/// it. Grants that name the old id do not follow it; the caller reissues them.
pub async fn rename_vault_item(
    target: &str,
    from: &str,
    to: &str,
    json: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", from)?;
    vault_word("vault item", to)?;
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: {from} could not be renamed to {to}: {detail}",
            resolved.name
        ))
    };
    // The usage literal, never the bare command name, for the reason
    // `retag_vault_item` gives: rustc packs literals into one blob.
    let capable = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "strings -a {} 2>/dev/null | grep -q 'usage: rename <id> <new-id>'",
            crate::deploy::shlex_quote(&skarbiec)
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !capable.ok() {
        return Err(refused(format!(
            "the Skarbiec build at {skarbiec} predates the rename operation"
        )));
    }
    let source = read_vault_phase(&resolved, &vault, from, &runner)
        .await
        .map_err(refused)?;
    if source.state == "absent" {
        return Err(refused(format!("{vault} holds no item {from}")));
    }
    let occupied = read_vault_phase(&resolved, &vault, to, &runner)
        .await
        .map_err(refused)?;
    if occupied.state != "absent" {
        return Err(refused(format!(
            "{vault} already holds {to} (rev={} state={}); nothing was renamed",
            occupied.revision, occupied.state
        )));
    }
    let renamed = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} rename {} {} > /dev/null",
            crate::deploy::shlex_quote(&gnupg_home),
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(from),
            crate::deploy::shlex_quote(to),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !renamed.ok() {
        return Err(refused(crate::deploy::host_channel::last_error_line(
            &renamed,
            "remote rename failed",
        )));
    }
    let after = read_vault_phase(&resolved, &vault, to, &runner)
        .await
        .map_err(refused)?;
    let gone = read_vault_phase(&resolved, &vault, from, &runner)
        .await
        .map_err(refused)?;
    if after.state == "absent" || gone.state != "absent" {
        return Err(refused(format!(
            "after the rename {to} is {} and {from} is {}",
            after.state, gone.state
        )));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "from": from,
                "to": to,
                "revision": after.revision,
                "state": after.state,
                "tags": after.tags,
            }))?
        );
    } else {
        println!(
            "{}: {from} is now {to} (rev={} state={} tags={})",
            resolved.name, after.revision, after.state, after.tags
        );
    }
    Ok(())
}

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
    let moved = move_vault_item(target, from, to).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&moved.report())?);
    } else {
        println!(
            "{}: {from} is now {to} (rev={} state={} tags={})",
            moved.target, moved.revision, moved.state, moved.tags
        );
    }
    Ok(())
}

/// What a rename left on the host.
pub(crate) struct MovedItem {
    pub(crate) target: String,
    pub(crate) from: String,
    pub(crate) to: String,
    revision: String,
    state: String,
    tags: String,
}

impl MovedItem {
    pub(crate) fn report(&self) -> serde_json::Value {
        json!({
            "target": self.target,
            "from": self.from,
            "to": self.to,
            "revision": self.revision,
            "state": self.state,
            "tags": self.tags,
        })
    }
}

/// The rename itself, reported instead of printed, for a caller whose own
/// output is one document (the release publisher declaration moves a
/// publisher bearer minted under a random id to the item named after its
/// product this way).
pub(crate) async fn move_vault_item(
    target: &str,
    from: &str,
    to: &str,
) -> Result<MovedItem, CmdError> {
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
        CmdError::refused(format!(
            "{}: {from} could not be renamed to {to}: {detail}",
            resolved.name
        ))
    };
    // A read of the vault fails for the vault's reason, not as a refusal.
    let reading = |error: CmdError| {
        error.within(format!(
            "{}: {from} could not be renamed to {to}",
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
    .map_err(CmdError::from)?;
    if !capable.ok() {
        return Err(refused(format!(
            "the Skarbiec build at {skarbiec} predates the rename operation"
        )));
    }
    let source = read_vault_phase(&resolved, &vault, from, &runner)
        .await
        .map_err(reading)?;
    if source.state == "absent" {
        return Err(CmdError::missing(format!(
            "{}: {from} could not be renamed to {to}: {vault} holds no item {from}",
            resolved.name
        )));
    }
    let occupied = read_vault_phase(&resolved, &vault, to, &runner)
        .await
        .map_err(reading)?;
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
    .map_err(CmdError::from)?;
    if !renamed.ok() {
        return Err(refused(crate::deploy::host_channel::last_error_line(
            &renamed,
            "remote rename failed",
        )));
    }
    let after = read_vault_phase(&resolved, &vault, to, &runner)
        .await
        .map_err(reading)?;
    let gone = read_vault_phase(&resolved, &vault, from, &runner)
        .await
        .map_err(reading)?;
    if after.state == "absent" || gone.state != "absent" {
        return Err(refused(format!(
            "after the rename {to} is {} and {from} is {}",
            after.state, gone.state
        )));
    }
    Ok(MovedItem {
        target: resolved.name.clone(),
        from: from.to_string(),
        to: to.to_string(),
        revision: after.revision,
        state: after.state,
        tags: after.tags,
    })
}

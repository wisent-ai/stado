use serde_json::json;

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::item::read_vault_phase;
use crate::cli::host::secrets::vault::vault_word;

/// Replace one Skarbiec item's tags on TARGET, and report what the host had
/// before and has after.
///
/// Tags decide who may spend a credential: Brama treats a vault item as a
/// subscription only when it carries `brama:subscription` and
/// `brama:agent:<agent>`, so an item that loses them leaves the fleet while
/// remaining perfectly valid — silently, because a credential nobody can see
/// still passes every check that counts credentials. Restoring them is a write
/// only the owner key can make, and that key lives on the host, so this runs
/// there and nowhere else.
///
/// Tags only: the payload is never read, rewritten or re-encrypted, which is
/// the whole reason this is not a `set-json`.
pub async fn retag_vault_item(
    target: &str,
    item: &str,
    tags: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    if let Some(tags) = tags {
        for tag in tags.split(',') {
            vault_word("tag", tag)?;
        }
    }
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;

    // A remote refusal names the check that failed, in the words the retired
    // script printed to stderr.
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: {item} could not be retagged: {detail}",
            resolved.name
        ))
    };
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-x {}", crate::deploy::shlex_quote(&skarbiec)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no Skarbiec binary at {skarbiec}")));
    }
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-f {}", crate::deploy::shlex_quote(&vault)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no vault at {vault}")));
    }
    // Whether this build can retag at all. The discriminator is the usage
    // literal, never the bare command name: rustc packs string literals into
    // one unterminated blob, so a binary that carries the command shows
    // `...setgetretagdelete...` on a single line and a whole-line match for
    // `retag` reports absent on a build that has it. That false negative cost
    // an hour and sent one diagnosis at the wrong host.
    let capable = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "strings -a {} 2>/dev/null | grep -q 'usage: retag <id> --tags'",
            crate::deploy::shlex_quote(&skarbiec)
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !capable.ok() {
        return Err(refused(format!(
            "the Skarbiec build at {skarbiec} predates the retag operation"
        )));
    }

    // The caller states what the host had and has rather than asserting
    // success: read the item before, retag, read it again.
    let before = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(refused)?;
    // No --tags: this is a read. Report what the host holds and write nothing,
    // so the operator who is about to replace a tag list can see the list they
    // would be replacing.
    let Some(tags) = tags else {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": resolved.name,
                    "item": item,
                    "read_only": true,
                    "state": before.state,
                    "revision": before.revision,
                    "tags": before.tags,
                }))?
            );
        } else {
            println!(
                "{}: {item} has rev={} state={} tags={}",
                resolved.name, before.revision, before.state, before.tags
            );
        }
        return Ok(());
    };
    let retagged = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} retag {} --tags {} > /dev/null",
            crate::deploy::shlex_quote(&gnupg_home),
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(tags),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !retagged.ok() {
        return Err(refused(crate::deploy::host_channel::last_error_line(
            &retagged,
            "remote retag failed",
        )));
    }
    let after = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(|detail| {
            CmdError::click(format!(
                "{}: {item} reported no tags after the retag; the host said: {detail}",
                resolved.name
            ))
        })?;
    let before = Some(before);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "before": before.as_ref().map(|phase| json!({
                    "state": phase.state,
                    "revision": phase.revision,
                    "tags": phase.tags,
                })),
                "after": {
                    "state": after.state,
                    "revision": after.revision,
                    "tags": after.tags,
                },
            }))?
        );
    } else {
        if let Some(phase) = &before {
            println!(
                "{}: {item} had rev={} state={} tags={}",
                resolved.name, phase.revision, phase.state, phase.tags
            );
        }
        println!(
            "{}: {item} now rev={} state={} tags={}",
            resolved.name, after.revision, after.state, after.tags
        );
    }
    Ok(())
}

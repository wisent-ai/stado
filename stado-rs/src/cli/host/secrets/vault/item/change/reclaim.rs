use serde_json::json;

use crate::cli::CmdError;

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::host::secrets::vault::vault_word;

/// Return one item from an external writer's control to the owner of TARGET's
/// vault, with Skarbiec's own `reclaim`, on the host whose owner key can
/// write it.
///
/// An item a consumer wrote (Stado's own `stado-secret` items among them) is
/// controlled by that writer, so every owner change — `retag`, `rename`,
/// `delete` — refused it with "<item> is not owner-controlled". That left
/// such an item unable to carry the `stado:role:<role>` tag readers now find
/// it by: Brama's launcher reads `wisent-backend-model-router` by role, and
/// the item holding it could not be tagged. Only control moves; no field,
/// tag or revision changes. Skarbiec refuses items a credential lifecycle or
/// Weles controls.
pub async fn reclaim_vault_item(
    target: &str,
    item: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    let (resolved, report) =
        remote_skarbiec_json(target, &["reclaim".into(), item.to_string()]).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "status": "owner-controlled",
                "skarbiec": report,
            }))?
        );
    } else {
        println!("{}: {item} is owner-controlled", resolved.name);
    }
    Ok(())
}

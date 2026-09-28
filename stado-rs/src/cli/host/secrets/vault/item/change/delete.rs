use serde_json::json;

use crate::cli::CmdError;

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::host::secrets::vault::vault_word;

/// Delete one owner-controlled item from TARGET's vault with Skarbiec's own
/// `delete`, on the host whose owner key can write it.
///
/// For items whose product or role is retired: a grant that can still read a
/// retired item keeps a retired identity alive, and nothing else could remove
/// the item from the owner vault. Skarbiec refuses items a credential
/// lifecycle or Weles controls, and keeps the deletion restorable.
pub async fn delete_vault_item(
    target: &str,
    item: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    let (resolved, report) =
        remote_skarbiec_json(target, &["delete".into(), item.to_string()]).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "status": "deleted",
                "skarbiec": report,
            }))?
        );
    } else {
        println!("{}: deleted {item}", resolved.name);
    }
    Ok(())
}

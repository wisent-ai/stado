//! An item a declaration names and a role readers ask for, kept as one item:
//! the named item is the role's one live holder.

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::host::secrets::vault::vault_word;
use crate::skarbiec::{roles, ItemInfo};

use super::put::write_vault_item;

/// Keep the item named ITEM as the one item playing ROLE in TARGET's vault,
/// minting it from PAYLOAD when the vault holds neither. A declaration that
/// names its item needs both: a release publisher is read as the item named
/// after its product, while role reads and grants ask for the role. Minting
/// under a random id satisfied only the role, so the declaration guard then
/// refused the publisher just minted. A lone holder under another
/// id is renamed ITEM, keeping its bearer and history; an ITEM without the
/// role tag gains it beside its other tags. ITEM held while another item
/// plays the role is refused: which bearer readers should get is a guess.
pub(crate) async fn write_named_role_item(
    target: &str,
    item: &str,
    role: &str,
    item_type: &str,
    payload: &str,
) -> Result<Value, CmdError> {
    vault_word("vault item", item)?;
    vault_word("role", role)?;
    let (_, listing) = remote_skarbiec_json(target, &["list".into()]).await?;
    let items: Vec<ItemInfo> = serde_json::from_value(listing).map_err(|error| {
        CmdError::click(format!(
            "{target}: Skarbiec list did not answer items: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let tag = roles::role_tag(role);
    let named = items
        .iter()
        .find(|candidate| candidate.id == item && candidate.deleted != Some(true));
    match (roles::holders(&items, role).as_slice(), named) {
        ([one], _) if one.id == item => {
            Ok(json!({ "created": false, "target": target, "item": item, "kind": item_type }))
        }
        ([], None) => write_vault_item(target, item, item_type, payload, true, Some(&tag)).await,
        ([one], None) => {
            let moved = super::change::rename::move_vault_item(target, &one.id, item).await?;
            Ok(json!({
                "created": false, "target": target, "item": item, "renamed": moved.report(),
            }))
        }
        ([], Some(untagged)) => {
            let tags = untagged
                .tags
                .iter()
                .flatten()
                .map(String::as_str)
                .chain([tag.as_str()])
                .collect::<Vec<_>>()
                .join(",");
            let host = credential_host(target).await?;
            let runner = crate::deploy::production_runner();
            let skarbiec =
                crate::cli::host::release_managed_skarbiec(&host.target, &runner, &host.home)
                    .await?;
            super::change::retag::run_retag(
                &host.target,
                &host.gnupg_home,
                &host.vault,
                &skarbiec,
                item,
                &tags,
                &runner,
            )
            .await?
            .map_err(|detail| {
                CmdError::refused(format!("{target}: {item} could not take {tag}: {detail}"))
            })?;
            Ok(json!({ "created": false, "target": target, "item": item, "tagged": tag }))
        }
        (holders, _) => Err(CmdError::refused(format!(
            "{target}: {} plays role {role} while release readers ask for item {item}; exactly one \
             item may hold it, and it must be {item}. Rename or retag the others with `stado \
             credentials item rename|retag --host {target}` and declare again",
            holders
                .iter()
                .map(|holder| holder.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Every name among ITEMS whose item does not stand as the one live item
/// playing the role of the same name in TARGET's vault, from one listing.
///
/// A release publisher is declared by item name and the verifier grant asks
/// for the role of that name, so an item minted under a random id with the
/// tag, or a product-named item without it, satisfies one reader and fails
/// the other. [`write_named_role_item`] puts either shape right; this says
/// which names need it without one vault listing per name.
pub(crate) async fn named_role_items_out_of_shape(
    target: &str,
    items: &[String],
) -> Result<Vec<String>, CmdError> {
    let (_, listing) = remote_skarbiec_json(target, &["list".into()]).await?;
    let listed: Vec<ItemInfo> = serde_json::from_value(listing).map_err(|error| {
        CmdError::click(format!(
            "{target}: Skarbiec list did not answer items: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    Ok(items
        .iter()
        .filter(|item| {
            !matches!(
                roles::holders(&listed, item).as_slice(),
                [one] if one.id == item.as_str()
            )
        })
        .cloned()
        .collect())
}

//! Whether the vault a catalog was registered against can answer it.
//!
//! A row `consumer|role:<role>|field` registers `acquire:role:<role>#<field>`,
//! and Skarbiec redeems it from the one live item tagged `stado:role:<role>`.
//! Registration succeeds whether or not any item plays the role, so a catalog
//! that switched its rows from item names to roles registered cleanly against
//! a vault none of whose items carried a role tag, and the service then failed
//! on its first read with `acquisition field does not exist on item`. This
//! reads the vault after registering and settles each role:
//!
//! - played by one live item: nothing to do;
//! - played by none, while a live item whose id is the role and which plays no
//!   role exists: that is the item the row named before rows named roles
//!   (every row kept its item's name as the role), and it is given the role
//!   tag, its other tags kept;
//! - otherwise the role is named in the answer as unheld, or as contested when
//!   several items play it, so the deploy that registered it says which reads
//!   will fail instead of the service finding out at startup.

use serde_json::{json, Value};

use crate::cli::host::machine::users::credentials::CredentialHost;
use crate::cli::host::secrets::vault::item::change::retag::run_retag;
use crate::cli::host::secrets::vault::mirror::read::remote_skarbiec_json_at;
use crate::cli::CmdError;
use crate::skarbiec::roles::{holders, role_tag, ROLE_TAG_PREFIX};
use crate::skarbiec::ItemInfo;

/// The roles a catalog's rows name, each once, in catalog order. Rows that
/// name an item rather than a role have nothing to settle.
fn catalog_roles(catalog: &str) -> Vec<String> {
    let mut roles: Vec<String> = Vec::new();
    for line in catalog.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(role) = line
            .split('|')
            .nth(1)
            .and_then(|item| item.strip_prefix("role:"))
        else {
            continue;
        };
        if !roles.iter().any(|known| known == role) {
            roles.push(role.to_string());
        }
    }
    roles
}

/// The live item a row named before it named ROLE: same id, playing no role.
fn former_item<'a>(items: &'a [ItemInfo], role: &str) -> Option<&'a ItemInfo> {
    items.iter().find(|item| {
        item.id == role
            && item.deleted != Some(true)
            && !item
                .tags
                .iter()
                .flatten()
                .any(|tag| tag.starts_with(ROLE_TAG_PREFIX))
    })
}

/// Settle every role the registered CATALOG (a path on the host) names
/// against HOST's owner vault, and answer what was found and done.
pub(super) async fn settle_role_holders(
    host: &CredentialHost,
    catalog: &str,
    runner: &crate::deploy::Runner,
) -> Result<Value, CmdError> {
    let name = &host.target.name;
    let text = crate::deploy::host_channel::remote_read_file(&host.target, catalog, runner)
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::unreachable(format!(
                "{name}: the registered catalog {catalog} could not be read back"
            ))
        })?;
    let roles = catalog_roles(&text);
    let mut held = 0usize;
    let mut adopted = Vec::new();
    let mut unheld = Vec::new();
    let mut contested = Vec::new();
    let mut unadopted = Vec::new();
    if !roles.is_empty() {
        let (_, listing) =
            remote_skarbiec_json_at(name, &["list".to_string()], None, None, None).await?;
        let items: Vec<ItemInfo> = serde_json::from_value(listing).map_err(|error| {
            CmdError::click(format!(
                "{name}: skarbiec list did not answer item metadata: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let skarbiec =
            crate::cli::host::release_managed_skarbiec(&host.target, runner, &host.home).await?;
        for role in roles {
            match holders(&items, &role).len() {
                1 => held += 1,
                0 => match former_item(&items, &role) {
                    Some(item) => {
                        let mut tags = item.tags.clone().unwrap_or_default();
                        tags.push(role_tag(&role));
                        let retagged = run_retag(
                            &host.target,
                            &host.gnupg_home,
                            &host.vault,
                            &skarbiec,
                            &item.id,
                            &tags.join(","),
                            runner,
                        )
                        .await?;
                        // A refusal settles this role only: an item an
                        // external writer controls is that writer's to tag,
                        // and the other roles still deserve their holders.
                        match retagged {
                            Ok(()) => adopted.push(role),
                            Err(reason) => unadopted.push(json!({
                                "role": role,
                                "item": item.id,
                                "reason": reason.trim(),
                            })),
                        }
                    }
                    None => unheld.push(role),
                },
                _ => contested.push(role),
            }
        }
    }
    for role in &unheld {
        eprintln!(
            "{name}: no item plays role {role}, so every read of role:{role} is refused; store it \
             with `stado credentials item put --host {name} --role {role}`"
        );
    }
    for refused in &unadopted {
        eprintln!(
            "{name}: no item plays role {}, and its former item {} could not be given the tag {}: \
             {}; until it carries the tag every read of role:{} is refused",
            refused["role"].as_str().unwrap_or_default(),
            refused["item"].as_str().unwrap_or_default(),
            role_tag(refused["role"].as_str().unwrap_or_default()),
            refused["reason"].as_str().unwrap_or_default(),
            refused["role"].as_str().unwrap_or_default(),
        );
    }
    for role in &contested {
        eprintln!(
            "{name}: several items carry {}; exactly one may play role {role}, so its reads are \
             refused until the others are retagged",
            role_tag(role)
        );
    }
    Ok(json!({
        "held": held,
        "adopted": adopted,
        "unadopted": unadopted,
        "unheld": unheld,
        "contested": contested,
    }))
}

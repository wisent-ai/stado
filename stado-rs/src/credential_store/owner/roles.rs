//! Naming the role an owner-vault item plays, beside the tags it already has.

use super::{binary, list_items, vault};
use crate::skarbiec::SkarbiecError;

/// Make item `id` play `role` in the resolved owner vault: the role tag is
/// added beside the item's other tags, so a release reader that names the
/// role finds the item a declaration writes under its own id. An item that
/// already plays it is left alone; another item playing it is refused,
/// because which one readers should get is a guess.
pub fn name_role(id: &str, role: &str) -> Result<(), SkarbiecError> {
    let items = list_items()?;
    let tag = crate::skarbiec::roles::role_tag(role);
    let holders = crate::skarbiec::roles::holders(&items, role);
    if holders.iter().any(|holder| holder.id == id) {
        return Ok(());
    }
    if let Some(other) = holders.first() {
        return Err(SkarbiecError::Deployment(format!(
            "{} plays role {role} while {id} is written for it; exactly one item may carry {tag}",
            other.id
        )));
    }
    let item = items
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| SkarbiecError::Deployment(format!("{id} is not in the owner vault")))?;
    let tags = item
        .tags
        .iter()
        .flatten()
        .map(String::as_str)
        .chain([tag.as_str()])
        .collect::<Vec<_>>()
        .join(",");
    let output = crate::wait::output(
        std::process::Command::new(binary()?)
            .arg("retag")
            .arg(id)
            .arg("--tags")
            .arg(&tags)
            .env("SKARBIEC_VAULT_FILE", vault()?)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
    .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not tag {id} with {tag}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

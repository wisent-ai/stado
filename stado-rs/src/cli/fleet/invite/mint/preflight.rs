//! The refusal that runs before anything is minted: a target name two
//! machines could end up sharing.

use serde_json::Value;

use crate::cli::fleet::invite::record::{Invite, STATUS_OPEN};

/// Refuse a target name already taken by a registered machine or by a live
/// invite. Silently suffixing a colliding name is how two machines end up
/// sharing one channel key. Pure.
pub fn preflight_invite_name(
    document: &Value,
    live: &[(Invite, &'static str)],
    name: &str,
) -> Result<(), String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!(
            "target '{name}' is already registered; invite a different name with --name"
        ));
    }
    if let Some((invite, _)) = live
        .iter()
        .find(|(invite, status)| *status == STATUS_OPEN && invite.target_name == name)
    {
        return Err(format!(
            "invite {} is already open for target '{name}'; revoke it or use --name",
            invite.id
        ));
    }
    Ok(())
}

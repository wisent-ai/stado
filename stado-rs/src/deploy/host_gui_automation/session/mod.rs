use super::*;

mod declared;
mod identity;
mod invoke;

pub(in crate::deploy::host_gui_automation) use declared::{
    automates_declared_session, declared_gui_bindings, require_declared_session,
};
pub(in crate::deploy::host_gui_automation) use identity::{
    app_identity, apple_challenge_helper_path, helper_identity, login_user,
};
pub(in crate::deploy::host_gui_automation) use invoke::{
    gui_user_id, invoke_as_gui_user, invoke_in_gui_session, optional, optional_sudo,
    remove_if_present, run, run_as_gui_user, run_in_gui_session, run_sudo,
};

pub(in crate::deploy::host_gui_automation) fn require_target(
    target: &ComputeTarget,
) -> Result<(), DeployError> {
    if !target.has_ssh_connection() {
        return Err(DeployError(format!(
            "target {} has no SSH connection path in the registry",
            target.name
        )));
    }
    if target.release_platform != "darwin-arm64" {
        return Err(DeployError(format!(
            "target {} is {:?}; GUI automation requires darwin-arm64",
            target.name, target.release_platform
        )));
    }
    Ok(())
}

pub(in crate::deploy::host_gui_automation) fn safe_identity(
    value: &str,
    kind: &str,
) -> Result<(), DeployError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DeployError(format!("invalid {kind} {value:?}")));
    }
    Ok(())
}

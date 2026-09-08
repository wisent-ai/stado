//! Whether the fleet can act inside the session a binding names.

use crate::cli::identity::APPLE_ACCOUNT;
use crate::targets::{ComputeTarget, IdentityBinding};

/// Can the fleet act inside the session this binding names?
///
/// A per-user identity is only usable where its own session can be driven: a two-factor
/// notification for an Apple account is delivered into the session of the user signed
/// into it, and no other session on that Mac can read or answer it.
///
/// The full GUI verdict matters. Merely matching the console user used to return true
/// while Accessibility was not granted and the CuaDriver runtime was absent. That is
/// not a drivable session; it is a correctly named session with no working actuator.
pub(in crate::cli::identity) async fn drivable_session(
    kind: &str,
    target: &ComputeTarget,
    binding: &IdentityBinding,
) -> Option<bool> {
    let declared = binding.user.as_deref()?;
    let password = super::service::host_sudo_password(target).await.ok()?;
    let runner = crate::deploy::production_runner();
    if kind == APPLE_ACCOUNT {
        crate::deploy::host_gui_automation::apple_challenge_session_ready_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
        .ok()
    } else {
        crate::deploy::host_gui_automation::automated_session_ready_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
        .ok()
    }
}

use super::*;

/// The macOS users this host's registry identity bindings name, each with the identity
/// it holds.
///
/// `IdentityBinding::user` exists because these identities are per-user: an Apple
/// account signed into one macOS user does not make the Mac's other users trusted. A
/// notification for it is delivered into that user's session and is unreadable from
/// every other one.
pub(in crate::deploy::host_gui_automation) fn declared_gui_bindings(
    target: &ComputeTarget,
) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = Vec::new();
    for binding in &target.identities {
        let Some(user) = binding.user.as_deref() else {
            continue;
        };
        if user.is_empty() || named.iter().any(|(existing, _)| existing == user) {
            continue;
        }
        named.push((user.to_string(), binding.identity.clone()));
    }
    named
}

/// Is the session we are about to automate one the registry declares an identity in?
pub(in crate::deploy::host_gui_automation) fn automates_declared_session(
    target: &ComputeTarget,
    user: &str,
) -> bool {
    let declared = declared_gui_bindings(target);
    declared.is_empty() || declared.iter().any(|(named, _)| named == user)
}

/// Refuse to enable automation for a session that holds none of the declared
/// identities.
///
/// Without this the resolution is silent and plausible: `login_user` answers with
/// whoever is at `/dev/console`, every step succeeds against that user, and `status`
/// ends with `gui-ready yes`. On charless-mac-mini on 2026-09-04 that sentence was
/// true about the `charles` session and useless about the fleet: the Apple account the
/// registry places there is signed into `controlyourai-relay`, whose prompts the
/// `charles` session cannot see. Enabling the wrong session is not partial progress
/// towards reading a code; it is a certainty of never reading one.
pub(in crate::deploy::host_gui_automation) fn require_declared_session(
    target: &ComputeTarget,
    user: &str,
) -> Result<(), DeployError> {
    if automates_declared_session(target, user) {
        return Ok(());
    }
    let named = declared_gui_bindings(target)
        .iter()
        .map(|(named, identity)| format!("{named} holds {identity}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(DeployError(format!(
        "{}: the GUI session available for automation is {user}'s, and this host's \
         registry declares {named}. An identity signed into one macOS user is invisible \
         to the others, so automating {user} cannot read a prompt for it. Put the \
         declared user at the console, or correct the host's identity binding.",
        target.name
    )))
}

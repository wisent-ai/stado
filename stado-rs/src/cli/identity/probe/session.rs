//! Whether the fleet can act inside the session a binding names, and when it
//! cannot, what stood in the way.

use crate::cli::identity::APPLE_ACCOUNT;
use crate::targets::{ComputeTarget, IdentityBinding};

/// The answer to "can the fleet act in this session", with its reason.
///
/// `drivable` is `None` when the host could not be asked, never conflated
/// with `false`. `reason` is what was actually observed: the item that
/// disagreed, or the error that stopped the probe. Until 2026-09-19 the
/// probe folded every failure into `None` with `.ok()`, so the Developer ID
/// relay on charless-mac-mini could only say "none of those Apple challenge
/// sessions is drivable" about a laptop that, asked directly, answered
/// `drivable` - and nothing on either side said which of the two probes was
/// wrong or why.
#[derive(Debug, Clone)]
pub(in crate::cli::identity) struct Drivability {
    pub drivable: Option<bool>,
    pub reason: Option<String>,
}

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
) -> Drivability {
    let Some(declared) = binding.user.as_deref() else {
        return Drivability {
            drivable: None,
            reason: Some("the binding names no macOS user".to_owned()),
        };
    };
    let password = match super::service::host_sudo_password(target).await {
        Ok(password) => password,
        Err(error) => {
            return Drivability {
                drivable: None,
                reason: Some(format!("host account password unreadable: {error}")),
            }
        }
    };
    let runner = crate::deploy::production_runner();
    let readiness = if kind == APPLE_ACCOUNT {
        crate::deploy::host_gui_automation::apple_challenge_session_readiness_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
    } else {
        crate::deploy::host_gui_automation::automated_session_readiness_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
    };
    match readiness {
        Ok(verdict) => Drivability {
            drivable: Some(verdict.ready),
            reason: verdict.reason,
        },
        Err(error) => Drivability {
            drivable: None,
            reason: Some(format!("session probe failed: {error}")),
        },
    }
}

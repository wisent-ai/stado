//! The verbs `stado identity` publishes, and the row shape they share.

use serde_json::{json, Value};

use super::probe::{
    drivable_session, is_local_target, local_apple_accounts, observe_apple_accounts,
    observe_user_apple_accounts, probes_own_user,
};
use super::APPLE_ACCOUNT;
use crate::targets::{ComputeTarget, IdentityBinding, Registry};

mod apple;
mod list;
mod verify;

pub use apple::{issue_apple_capabilities, relay_apple_challenge};
pub use list::list;
pub use verify::verify;

fn binding_row(
    target: &ComputeTarget,
    binding: &IdentityBinding,
    observed: Option<bool>,
    drivable: Option<bool>,
) -> Value {
    json!({
        // The registry's name for the machine, and deliberately not an address.
        // Callers route work to the holder through the registry channel, which
        // resolves the target itself; handing them a destination as well would be a
        // second way to say where a host is, and the one that goes stale -- which is
        // exactly how the hand-written APPLE_2FA_MAC_* variables came to exist.
        "host": target.name,
        "kind": binding.kind,
        "identity": binding.identity,
        "user": binding.user,
        "declared": true,
        "observed": observed,
        // Whether the fleet can act in this binding's session. `null` when the host
        // could not be asked, and never conflated with `false`.
        "drivable_session": drivable,
        "verified_at": binding.verified_at,
    })
}

struct Verification {
    rows: Vec<Value>,
    satisfied: bool,
}

async fn verified_bindings(registry: &Registry, kind: &str, identity: &str) -> Verification {
    let mut rows = Vec::new();
    let mut satisfied = false;
    for target in &registry.targets {
        for binding in &target.identities {
            if binding.kind != kind || binding.identity != identity {
                continue;
            }
            let observed = match binding.kind.as_str() {
                // Reading the machine we are already running on needs neither SSH nor
                // sudo only when the binding names the channel's login user. A binding
                // for another local user still needs the installed multi-user probe;
                // reading this process's preferences would confidently answer the
                // wrong account.
                APPLE_ACCOUNT if is_local_target(target) && probes_own_user(target, binding) => {
                    local_apple_accounts().map(|found| found.iter().any(|entry| entry == identity))
                }
                // The installed probe reads a named user's own preferences. Unknown
                // means the host could not be asked, never that the account is absent.
                APPLE_ACCOUNT if !probes_own_user(target, binding) => {
                    let user = binding.user.as_deref().unwrap_or_default();
                    observe_user_apple_accounts(&target.name, user)
                        .await
                        .map(|found| found.iter().any(|entry| entry == identity))
                }
                APPLE_ACCOUNT => observe_apple_accounts(&target.name)
                    .await
                    .map(|found| found.iter().any(|entry| entry == identity)),
                // An identity family we cannot probe stays unknown rather than being
                // reported as present.
                _ => None,
            };
            satisfied |= observed == Some(true);
            let drivable = drivable_session(kind, target, binding).await;
            rows.push(binding_row(target, binding, observed, drivable));
        }
    }
    Verification { rows, satisfied }
}

//! Where the origin an invite is built on comes from, and how deliberate that
//! answer was.

use crate::queue::JobStorage;

use super::probe_checkpoint;

/// Where the base address came from. The one-liner's usefulness depends on
/// this: an address from configuration is as durable as the deployment behind
/// it, and an address from a quick tunnel lasts exactly as long as that tunnel
/// does — which is a sentence the operator has to read, because the person who
/// runs the one-liner reads nothing at all.
pub const BASE_FROM_ENROLLMENT_URL: &str = "enrollment.url";
pub const BASE_FROM_INGRESS: &str = "ingress";
pub const BASE_FROM_API_URL: &str = "api.url";

/// Origin the invite probe and the printed one-liner are built from:
/// `enrollment.url` when configured, else `api.url`.
///
/// A deployment that publishes only the narrow `stado dashboard
/// --enrollment-only` listener has an enrollment origin that is not its
/// deployment endpoint, and the owner of a new machine can only reach the
/// former. Falling back to `api.url` keeps every existing deployment — which
/// serves enrollment from the same origin as everything else — unchanged.
pub fn enrollment_base() -> String {
    let enrollment = crate::config::enrollment_url();
    if enrollment.is_empty() {
        crate::config::stado_api_url()
    } else {
        enrollment
    }
}

/// The base an online invite is built on, in the order that puts the most
/// deliberate answer first.
///
/// 1. `enrollment.url`. Somebody configured an enrollment origin; nothing this
///    process discovers may override a decision that was written down.
/// 2. The published `enrollments/ingress.json`, **if its address still
///    answers**. This is the entrance `stado fleet ingress up` stood up, and it
///    is the whole reason the one-line mode is reachable on a fleet with no
///    public deployment. It is used only when it is live: a stale object from a
///    tunnel that has since closed must not become a one-liner, which is the
///    same rule the probe has always enforced, applied one step earlier.
/// 3. `api.url`, the deployment endpoint — unchanged, and still what every
///    fleet that serves enrollment from its own origin gets.
///
/// Returns the base and which of the three it is, so the caller can say out
/// loud that a tunnel address is temporary.
pub async fn resolve_invite_base(store: &JobStorage) -> (String, &'static str) {
    let configured = crate::config::enrollment_url();
    if !configured.is_empty() {
        return (configured, BASE_FROM_ENROLLMENT_URL);
    }
    if let Ok(Some(ingress)) = crate::cli::fleet::ingress::published(store).await {
        if probe_checkpoint(&ingress.base_url).await.reachable {
            return (ingress.base_url, BASE_FROM_INGRESS);
        }
    }
    (crate::config::stado_api_url(), BASE_FROM_API_URL)
}

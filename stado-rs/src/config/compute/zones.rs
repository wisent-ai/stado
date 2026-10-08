//! GCE zones the deployment rents in.

use std::sync::LazyLock;

use crate::config::resolve_capability_list_binding;

/// The declared zones (env `GCP_ZONES`, comma-separated), in preference
/// order. No list is built in: which zones carry a machine type, and which
/// quota a region holds, are facts of the deployment's own project, so the
/// operator states the zones and the provider refuses without them.
static ZONE_ROTATION: LazyLock<Vec<String>> = LazyLock::new(|| {
    resolve_capability_list_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "zones",
        &[],
    )
});

/// Zones, ordered by the deployment's preference. The provider iterates this
/// list and falls through GCE 'does not exist' / 'no capacity' errors until
/// one zone accepts the create_instance call.
pub fn zone_rotation() -> &'static [String] {
    &ZONE_ROTATION
}

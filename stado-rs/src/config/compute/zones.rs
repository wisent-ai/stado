//! GCE zone rotation, including per-machine-type overrides.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::config::region;

static ZONE_ROTATION: LazyLock<Vec<String>> = LazyLock::new(|| {
    let region = region();
    let mut zones = vec![
        format!("{region}-b"),
        format!("{region}-a"),
        format!("{region}-c"),
        format!("{region}-f"),
    ];
    zones.extend(
        [
            "europe-west4-a",
            "europe-west4-b",
            "europe-west4-c",
            "us-east1-c",
            "us-east1-d",
            "us-east4-a",
            "us-east4-b",
            "us-east4-c",
            "us-east5-a",
            "us-east5-b",
            "us-east5-c",
        ]
        .into_iter()
        .map(str::to_string),
    );
    zones
});

/// Zones, ordered by preference. Primary region's zones first (lowest
/// egress from existing infra in us-central1), then alternates. Provider
/// iterates this list and falls through GCE 'does not exist' / 'no
/// capacity' errors until one zone accepts the create_instance call.
pub fn zone_rotation() -> &'static [String] {
    &ZONE_ROTATION
}

static MACHINE_TYPE_ZONES: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    let region = region();
    HashMap::from([
        (
            "a2-ultragpu-1g".to_string(),
            vec![
                format!("{region}-c"),
                format!("{region}-a"),
                "us-east5-a".to_string(),
                "us-east5-b".to_string(),
                "europe-west4-a".to_string(),
                // Not europe-west4-b: the machine type is not present there.
                // Not us-east4-c: the NVIDIA_A100_80GB_GPUS regional quota is
                // 0 in us-east4, so it answers "Quota exceeded" every tick.
            ],
        ),
        (
            "a2-highgpu-1g".to_string(),
            vec![
                format!("{region}-b"),
                format!("{region}-a"),
                format!("{region}-c"),
                format!("{region}-f"),
                "europe-west4-a".to_string(),
                "europe-west4-b".to_string(),
                "us-east1-b".to_string(),
                // Not us-east1-c, us-east4-a or us-east4-b:
                // `gcloud compute machine-types describe a2-highgpu-1g --zone=...`
                // shows the SKU absent there, and a zone that answers
                // "Machine type does not exist" on every attempt wastes
                // dispatcher ticks and slows fleet ramp-up.
            ],
        ),
        (
            // nvidia-l4
            "g2-standard-4".to_string(),
            vec![
                format!("{region}-a"),
                format!("{region}-b"),
                format!("{region}-c"),
                "europe-west4-a".to_string(),
                "europe-west4-b".to_string(),
                "us-east1-c".to_string(),
                "us-east1-d".to_string(),
                "us-east4-a".to_string(),
                "us-east4-c".to_string(),
                // Not us-east4-b, us-east5-a or us-east5-b: g2-standard-4 is
                // absent there (`gcloud compute machine-types describe`), and
                // the dispatcher would log "Invalid machine type" each tick.
            ],
        ),
    ])
});

/// Per-machine-type zone rotation. Some SKUs don't exist in every zone, or
/// have regional spot-capacity quirks. For those buckets, list the zones
/// that actually carry the SKU first; the provider falls back to
/// [`zone_rotation`].
pub fn machine_type_zones() -> &'static HashMap<String, Vec<String>> {
    &MACHINE_TYPE_ZONES
}

//! `stado fleet catalog` — the declared catalog as the canonical registry
//! holds it, rendered as a table or as JSON.

use super::sections::{parse_channels, parse_enrollment};

/// `stado fleet catalog` — print the central catalog as declared in the
/// canonical registry.
pub async fn catalog(as_json: bool) -> Result<bool, String> {
    let document = crate::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let enrollment = parse_enrollment(&document)?;
    let channels = parse_channels(&document)?;
    if as_json {
        let rendered = serde_json::json!({
            "enrollment": {
                "declared": enrollment.declared,
                "allow_join": enrollment.allow_join,
                "allow_enroll": enrollment.allow_enroll,
                "allow_invite": enrollment.allow_invite,
                "allow_adopt": enrollment.allow_adopt,
                "require_verified_hostname": enrollment.require_verified_hostname,
            },
            "channels": {
                "declared": channels.declared,
                "control_plane": channels.control_plane,
                "notes": channels.notes,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    if !enrollment.declared && !channels.declared {
        println!("no enrollment catalog declared; every registration path is unrestricted");
        return Ok(true);
    }
    println!("enrollment:");
    println!(
        "  allow_join={} allow_enroll={} allow_invite={} allow_adopt={} require_verified_hostname={}",
        enrollment.allow_join,
        enrollment.allow_enroll,
        enrollment.allow_invite,
        enrollment.allow_adopt,
        enrollment.require_verified_hostname
    );
    if channels.declared {
        println!("channels:");
        for channel in &channels.control_plane {
            println!("  control_plane: {channel}");
        }
        if !channels.notes.is_empty() {
            println!("  notes: {}", channels.notes);
        }
    }
    Ok(true)
}

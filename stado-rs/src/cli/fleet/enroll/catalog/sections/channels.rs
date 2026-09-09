//! The `channels` section: how machines reach the control plane, kept in the
//! operator's own words.

use serde_json::Value;

/// The parsed `channels` section: declared channel names plus free-form
/// notes, preserving the operator's wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelsCatalog {
    pub declared: bool,
    pub control_plane: Vec<String>,
    pub notes: String,
}

/// Parse the optional `channels` section.
pub fn parse_channels(document: &Value) -> Result<ChannelsCatalog, String> {
    let Some(section) = document.get("channels") else {
        return Ok(ChannelsCatalog {
            declared: false,
            control_plane: Vec::new(),
            notes: String::new(),
        });
    };
    let section = section
        .as_object()
        .ok_or_else(|| "registry.channels: must be an object".to_string())?;
    let mut control_plane = Vec::new();
    if let Some(value) = section.get("control_plane") {
        let entries = value
            .as_array()
            .ok_or_else(|| "registry.channels.control_plane: must be an array".to_string())?;
        for (index, entry) in entries.iter().enumerate() {
            let channel = entry.as_str().ok_or_else(|| {
                format!("registry.channels.control_plane[{index}]: must be a string")
            })?;
            control_plane.push(channel.to_string());
        }
    }
    let notes = section
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(ChannelsCatalog {
        declared: true,
        control_plane,
        notes,
    })
}

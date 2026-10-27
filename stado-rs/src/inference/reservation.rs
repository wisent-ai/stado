use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reservation {
    pub deployment: String,
    pub target: String,
    pub gpu_mode: super::schema::GpuMode,
    pub engine: String,
    pub model: String,
    pub revision: String,
    pub endpoint_host: String,
    pub port: u16,
}

/// `~/.stado/inference/reservation.json` under the home every Stado file is
/// kept under ([`crate::config_file::home`]: HOME, else the user database),
/// so an agent unit started without HOME still sees the cards an engine
/// holds.
pub fn path() -> Result<PathBuf, String> {
    crate::config_file::home()
        .map(|home| home.join(".stado").join("inference").join("reservation.json"))
        .map_err(|cause| format!("the inference reservation cannot be found: {cause}"))
}

/// The inference reservation this host serves; `Ok(None)` only when none is
/// recorded. A record that cannot be read or parsed, a GPU mode included, is
/// an error naming the file and cause: read as "no reservation", it would
/// let GPU claims and a Vast listing onto cards an inference engine holds.
pub fn active() -> Result<Option<Reservation>, String> {
    let path = path()?;
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let reservation: Reservation =
        serde_json::from_str(&body).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((!reservation.deployment.trim().is_empty()).then_some(reservation))
}

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

pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".stado")
            .join("inference")
            .join("reservation.json")
    })
}

/// The inference reservation this host serves; `Ok(None)` only when none is
/// recorded. A record that cannot be read or parsed, a GPU mode included, is
/// an error naming the file and cause: read as "no reservation", it would
/// let GPU claims and a Vast listing onto cards an inference engine holds.
pub fn active() -> Result<Option<Reservation>, String> {
    let path = path().ok_or_else(|| {
        "HOME cannot be read, so the inference reservation cannot be found".to_string()
    })?;
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let reservation: Reservation =
        serde_json::from_str(&body).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((!reservation.deployment.trim().is_empty()).then_some(reservation))
}

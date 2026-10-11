use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reservation {
    pub deployment: String,
    pub target: String,
    pub gpu_mode: String,
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
/// recorded. A record that cannot be read or parsed is an error naming the
/// file and cause: read as "no reservation", it would let GPU claims and a
/// Vast listing onto cards an inference engine holds.
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
    let supported_mode = matches!(
        reservation.gpu_mode.as_str(),
        super::schema::GPU_EXCLUSIVE | super::schema::GPU_YIELDABLE
    );
    Ok((!reservation.deployment.trim().is_empty() && supported_mode).then_some(reservation))
}

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

pub fn active() -> Option<Reservation> {
    let body = std::fs::read_to_string(path()?).ok()?;
    let reservation: Reservation = serde_json::from_str(&body).ok()?;
    let supported_mode = matches!(
        reservation.gpu_mode.as_str(),
        super::schema::GPU_EXCLUSIVE | super::schema::GPU_YIELDABLE
    );
    (!reservation.deployment.trim().is_empty() && supported_mode).then_some(reservation)
}

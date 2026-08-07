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
    (!reservation.deployment.trim().is_empty()
        && reservation.gpu_mode == super::schema::GPU_EXCLUSIVE)
        .then_some(reservation)
}

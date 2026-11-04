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
    /// The board the engine holds, by driver UUID; None for a reservation
    /// that names none, which holds every board of the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

/// The variable a process started by this host's Stado (a peer-lending
/// worker) reads the host's reservation through: its own home is not the
/// host's, so without it the worker would see no reservation and offer the
/// board an engine holds.
pub const PATH_VARIABLE: &str = "STADO_INFERENCE_RESERVATION";

/// The reservation file: the path [`PATH_VARIABLE`] names when this process
/// was started with it, else `~/.stado/inference/reservation.json` under the
/// home every Stado file is kept under ([`crate::config_file::home`]: HOME,
/// else the user database), so an agent unit started without HOME still sees
/// the cards an engine holds.
pub fn path() -> Result<PathBuf, String> {
    if let Some(named) = std::env::var_os(PATH_VARIABLE).filter(|named| !named.is_empty()) {
        return Ok(PathBuf::from(named));
    }
    crate::config_file::home()
        .map(|home| {
            home.join(".stado")
                .join("inference")
                .join("reservation.json")
        })
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

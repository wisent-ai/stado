//! Machine-id resolution: the `WC_VAST_MACHINE_ID` override, the kernel
//! hostname read that backs auto-discovery, and the `/machines/?owner=me`
//! lookup that turns a hostname into the machine id every offer operation
//! is addressed to.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use crate::providers::vast::VastError;

use super::json::{jstr, machine_id_of, machines_of, py_value_str};
use super::VastClient;

/// The `WC_VAST_MACHINE_ID` env half of `_machine_id`, split out for tests:
/// stripped; empty -> None; non-int -> VastConfigError.
pub fn parse_machine_id_env(value: Option<&str>) -> Result<Option<i64>, VastError> {
    let Some(mid) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    mid.parse::<i64>()
        .map(Some)
        .map_err(|_| VastError::config(format!("WC_VAST_MACHINE_ID must be int: {mid}")))
}

/// The kernel's hostname, `gethostname(2)`: what `hostname(1)`, Python's
/// `socket.gethostname()` / `platform.node()` and Node's `os.hostname()`
/// answer, read without a child process. The call fails only for a buffer
/// too small for the name, and nix sizes it from the system's own limit, so a
/// failure is a broken platform: it stops here naming the errno instead of
/// answering an empty name every caller would take for another host.
pub fn system_hostname() -> String {
    match nix::unistd::gethostname() {
        Ok(name) => name.to_string_lossy().into_owned(),
        Err(errno) => panic!("gethostname(2) failed: {errno}"),
    }
}

impl VastClient {
    /// Resolve the machine id from the non-secret env override, else
    /// auto-discover it via `/machines/?owner=me` and hostname.
    pub async fn machine_id(&self) -> Result<i64, VastError> {
        self.machine_id_for_hostname(&system_hostname()).await
    }

    /// [`VastClient::machine_id`] with the hostname passed explicitly.
    pub async fn machine_id_for_hostname(&self, hostname: &str) -> Result<i64, VastError> {
        if let Some(mid) =
            parse_machine_id_env(std::env::var("WC_VAST_MACHINE_ID").ok().as_deref())?
        {
            return Ok(mid);
        }
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        let machines = machines_of(&resp);
        if machines.is_empty() {
            return Err(VastError::config(
                "Vast.ai /machines/?owner=me returned no machines. \
                 Register the box at https://cloud.vast.ai/host/setup first.",
            ));
        }
        for machine in &machines {
            if jstr(machine.get("hostname")).trim() == hostname {
                return machine_id_of(machine);
            }
        }
        if machines.len() == 1 {
            return machine_id_of(machines[0]);
        }
        let candidates: Vec<String> = machines
            .iter()
            .map(|m| {
                format!(
                    "{}={}",
                    py_value_str(m.get("id")),
                    py_value_str(m.get("hostname"))
                )
            })
            .collect();
        Err(VastError::config(format!(
            "Vast.ai returned {} machines and hostname '{hostname}' did not match any. \
             Set WC_VAST_MACHINE_ID explicitly. Candidates: {}",
            machines.len(),
            candidates.join(", ")
        )))
    }
}

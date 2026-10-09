//! What `release changes submit|list` is waiting on, said while it waits.
//!
//! Both commands read and write tickets in the job store. When that store
//! does not answer, they used to sit silent after the quality check, and the
//! session could not tell a slow store from a stuck one, or which store it
//! was. Each store request now names itself and the store on stderr when it
//! starts, and says how long it took when it ends, error or not.

use crate::cli::build_cmd::timing::{phase_of, Phase};
use crate::config;

/// The store `JobStorage::new` binds this process to, as its configuration
/// names it: the backend and its address.
fn store_named() -> String {
    let backend = config::wc_storage_backend();
    let address = match backend {
        "stado" => config::wc_stado_storage_url_configured().to_string(),
        "local" => config::wc_local_storage_path().to_string(),
        "" if !config::wc_local_storage_path().is_empty() => {
            return format!("local store {}", config::wc_local_storage_path());
        }
        _ => config::bucket().to_string(),
    };
    format!("{backend} store {address}")
}

/// Start one store request of `release changes <command>` and say so.
pub(super) fn store_request(command: &'static str, request: &str) -> Phase {
    phase_of(command, format!("{request} on the {}", store_named()))
}

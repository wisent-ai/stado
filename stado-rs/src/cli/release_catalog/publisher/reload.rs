//! Which processes `declare-publisher --reload HOST=SERVICE` may refresh.
//!
//! A reload refreshes the publisher table inside a running process, and the
//! release API that reads it runs only in the host Stado process. Any other
//! unit, including one that ran the Stado program under an older label, no
//! longer holds that table: reloading it would refresh nothing the release
//! API reads, or restart a unit that should not run.

use crate::cli::CmdError;
use crate::deploy::service_catalog;

/// `(host, unit)` for each `HOST=SERVICE`, refusing a malformed pair and any
/// unit that is not the host Stado process.
pub(super) fn reload_targets(reloads: &[String]) -> Result<Vec<(String, String)>, CmdError> {
    let host_unit = service_catalog::host_unit().map_err(CmdError::click)?;
    let mut targets = Vec::with_capacity(reloads.len());
    for pair in reloads {
        let (host, service) = pair
            .split_once('=')
            .ok_or_else(|| CmdError::usage(format!("--reload takes HOST=SERVICE, not {pair:?}")))?;
        if !service_catalog::is_host_unit(service).map_err(CmdError::click)? {
            return Err(CmdError::usage(format!(
                "--reload {service}: only the host Stado process ({host_unit}) serves the \
                 release API and caches the publisher table; reload {host_unit} instead"
            )));
        }
        targets.push((host.to_string(), service.to_string()));
    }
    Ok(targets)
}

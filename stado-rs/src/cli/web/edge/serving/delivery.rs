//! Delivering the configuration to the edge, the role of the host's one
//! Stado process (`com.wisent.stado`) that runs the reverse proxy.

use serde_json::{json, Value};

use super::super::{CADDYFILE_ON_EDGE, EDGE_ROLE, HOST_UNIT};
use super::{caddyfile, terminated_hostnames, CmdError};
use crate::config::WebApiEdge;
use crate::deploy::{production_runner, service, service_file_fetch};
use crate::targets::ComputeTarget;

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

/// The edge host's Stado process, as the registry declares it, proven to run
/// the edge role: its live argument vector, parsed as `stado serve` parses
/// it, carries `--edge-caddyfile`. A declaration that names the flag before
/// the process started with it proves nothing.
async fn proxy(edge: &WebApiEdge) -> Result<(ComputeTarget, service::ManagedService), CmdError> {
    let host = edge.target();
    let target = crate::cli::canonical_host(host).await?;
    let enable = format!(
        "the edge terminates nothing until {HOST_UNIT} on {host} runs its edge role: declare it \
         with `stado serve --edge-caddy <caddy program> --edge-caddyfile {CADDYFILE_ON_EDGE}` \
         and `stado service ensure stado --host {host}`, then reconcile with \
         `stado web edge hostnames`"
    );
    let declared = crate::cli::service::declared_matching(HOST_UNIT, Some(host))
        .await
        .map_err(|error| {
            let mut wrapped = CmdError::click(format!("{error}; {enable}"));
            wrapped.failure = error.failure;
            wrapped
        })?;
    let service = declared
        .into_iter()
        .next()
        .expect("declared_matching refuses an empty match");
    let runner = production_runner();
    match service::role_process(&target, &service, EDGE_ROLE, &runner).await {
        Ok((_, None)) => Ok((target, service)),
        Ok((_, Some(reason))) => Err(CmdError::refused(format!("{reason}; {enable}"))),
        Err(error) => Err(CmdError::click(format!(
            "{host}: whether {HOST_UNIT} runs the edge role could not be read: {error}"
        ))),
    }
}

/// Make the edge terminate exactly `routes`, and report what that changed.
///
/// `stado web route` calls this before it writes any DNS, and the reason is
/// the opposite of the obvious one: the certificate cannot be ordered yet.
/// Let's Encrypt delivers its challenge to whatever the hostname resolves to,
/// so Caddy can only obtain the certificate once the record points here. What
/// this step buys is that the site block already exists when it does — the
/// first request to arrive after the cutover finds a proxy that knows the
/// name, instead of one that has never heard of it and cannot even begin an
/// issuance. `apply` false reports the same comparison and writes nothing, on
/// the host or locally.
///
/// The desired set is passed in rather than read here because the caller knows
/// something the declarations do not yet: `stado web remove` retracts a
/// hostname while the product is still declared, so its route has to be
/// excluded by the caller that is removing it.
pub(in crate::cli::web) async fn deliver(
    edge: &WebApiEdge,
    routes: &[(String, Vec<String>)],
    apply: bool,
) -> Result<Value, CmdError> {
    let desired = caddyfile(edge, routes);
    let (target, declared) = proxy(edge).await?;
    let runner = production_runner();
    let installed = service_file_fetch::fetch_file(&target, CADDYFILE_ON_EDGE, &runner)
        .await
        .map_err(click)?;
    // A missing file is the first delivery, not a failure. Every other unread
    // state is: delivering over a configuration this process could not read
    // would report a change it cannot describe.
    let current = match installed.report.file_state.as_str() {
        service_file_fetch::FILE_MISSING => String::new(),
        service_file_fetch::FILE_READ if installed.ok() => {
            String::from_utf8_lossy(&installed.content).into_owned()
        }
        _ => {
            return Err(CmdError::click(format!(
                "the edge's current configuration could not be read, so nothing was delivered: {}",
                installed
                    .failure(&target.name)
                    .unwrap_or_else(|| "no detail".to_string())
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown))
        }
    };

    let terminates = terminated_hostnames(&current);
    let wanted: Vec<String> = routes
        .iter()
        .map(|(hostname, _)| hostname.clone())
        .collect();
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|hostname| !terminates.contains(hostname))
        .collect();
    let extra: Vec<&String> = terminates
        .iter()
        .filter(|hostname| !wanted.contains(hostname))
        .collect();
    let differs = installed.content != desired.as_bytes();
    let unit = declared.unit_id().to_string();
    let mut report = json!({
        "target": target.name,
        "address": edge.address(),
        "unit": unit,
        "path": CADDYFILE_ON_EDGE,
        "local_file": Value::Null,
        "hostnames": wanted,
        "terminated": terminates,
        "missing": missing,
        "unexpected": extra,
        "change": "unchanged",
    });

    if !differs {
        return Ok(report);
    }
    if !apply {
        report["change"] = json!("would-deliver");
        return Ok(report);
    }

    let local = write_local(&desired)?;
    let content = std::fs::read(&local)?;
    let synced = service::sync_service_file(&target, CADDYFILE_ON_EDGE, &content, 0o600, &runner)
        .await
        .map_err(click)?;
    if !synced.succeeded("file_synced") {
        return Err(CmdError::click(format!(
            "{}: the edge's configuration was not delivered: {}",
            target.name,
            synced.failure()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // No restart: the edge role runs Caddy with `--watch`, which loads the
    // file it was just given. Restarting com.wisent.stado would take every
    // other role on the host down with it for a configuration change.
    report["change"] = json!("delivered");
    report["local_file"] = json!(local.to_str());
    Ok(report)
}

/// The local copy of what was delivered.
///
/// `stado service file-sync` sends the bytes of a local file, and keeping that
/// file is what lets an operator read what the edge was sent without fetching
/// it back off the host. The bytes that travel are read back out of it, so the
/// copy and the delivery can never disagree.
fn write_local(text: &str) -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| {
        CmdError::click("HOME is not set, so there is nowhere to write the generated Caddyfile")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let directory = std::path::Path::new(&home).join(".stado").join("web-edge");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("Caddyfile");
    let staged = directory.join("Caddyfile.stado-web-edge");
    std::fs::write(&staged, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&staged, &path)?;
    Ok(path)
}

//! Delivering the configuration to the edge, through the reverse proxy the
//! registry declares as a managed unit.

use serde_json::{json, Value};

use super::super::{CADDYFILE_ON_EDGE, PROXY_UNIT};
use super::{caddyfile, terminated_hostnames, CmdError};
use crate::config::WebApiEdge;
use crate::deploy::{host_channel, production_runner, service, service_file_fetch};
use crate::targets::ComputeTarget;

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

/// The edge's reverse proxy, as the registry declares it.
async fn proxy(edge: &WebApiEdge) -> Result<(ComputeTarget, service::ManagedService), CmdError> {
    let unit = super::unit_label(PROXY_UNIT);
    let target = host_channel::canonical_target(edge.target())
        .await
        .map_err(click)?;
    let declared = crate::cli::service::declared_matching(&unit, Some(edge.target()))
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{error}; the edge terminates nothing until its reverse proxy is a managed unit: \
                 write a declaration naming {unit} on {host}, install it with \
                 `stado service declare --file <declaration>` and \
                 `stado service deploy {unit} --host {host}`, then reconcile with \
                 `stado web edge hostnames`",
                host = edge.target()
            ))
        })?;
    let service = declared
        .into_iter()
        .next()
        .expect("declared_matching refuses an empty match");
    Ok((target, service))
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
            )))
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
        )));
    }
    // The file on disk is not the configuration until the proxy has read it,
    // and a hostname whose certificate was never ordered is exactly the
    // outage this ordering exists to prevent.
    let restarted = service::restart_service(&target, &declared, &runner)
        .await
        .map_err(click)?;
    if !restarted.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{}: {unit} holds the new configuration on disk and did not restart, so it is still \
             serving the old one: {}",
            target.name,
            restarted.failure()
        )));
    }
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

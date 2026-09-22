//! The declared legacy unit, booted out on the way into the stable bind and
//! bootstrapped back on the way out of it.

use std::process::Command;

use crate::release_control::ReleaseTargetPolicy;

/// Only the declared predecessor may keep serving while a candidate starts
/// on its separate port. Reuse the service command's native owner reader,
/// including its parent-chain and launchd-domain checks.
pub(crate) fn owns_stable_bind(target: &ReleaseTargetPolicy, port: u16) -> Result<bool, String> {
    use crate::deploy::{service, service_serving};
    use std::io::Write;
    use std::process::Stdio;

    let (Some(label), Some(plist)) = (
        target.legacy_launchd_label.as_deref(),
        target.legacy_launchd_plist.as_deref(),
    ) else {
        return Ok(false);
    };
    // stop_legacy addresses the system domain; never admit a user job that
    // the existing cutover cannot stop and restore.
    if !cfg!(target_os = "macos")
        || service::UnitDomain::from_path(plist) != service::UnitDomain::System
    {
        return Ok(false);
    }
    let script = service::serving_script(
        label,
        plist,
        &service_serving::remote_serving_script(&[port]),
    )
    .map_err(|error| format!("cannot prepare legacy ownership read: {error}"))?;
    let mut child = Command::new("/bin/bash")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start legacy ownership read for {label}: {error}"))?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| format!("legacy ownership reader for {label} has no stdin"))?
        .write_all(script.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot finish legacy ownership read for {label}: {error}"))?;
    input.map_err(|error| format!("cannot send legacy ownership read for {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "legacy ownership read for {label} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let report = service_serving::parse_serving(&String::from_utf8_lossy(&output.stdout))
        .map_err(|error| format!("cannot read legacy ownership for {label}: {error}"))?;
    Ok(report.unit == label
        && report.unit_path == plist
        && report.loaded == "yes"
        && report.listeners_state == service_serving::LISTENERS_READ
        && report.ports.len() == 1
        && report.ports[0].port == port
        && !report.ports[0].holders.is_empty()
        && report.ports[0].holders.iter().all(|holder| {
            holder.owner_state == service_serving::OWNER_RESOLVED && holder.owner == label
        }))
}

pub(crate) fn stop_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(label) = target.legacy_launchd_label.as_deref() else {
        return Ok(());
    };
    let status = Command::new("/usr/bin/sudo")
        .args([
            "-n",
            "/bin/launchctl",
            "bootout",
            &format!("system/{label}"),
        ])
        .status()
        .map_err(|error| format!("cannot disable legacy launchd service {label}: {error}"))?;
    // This asks for a state, not an action: the legacy unit must not hold the
    // port before the proxy binds it. launchd answers 113 ("Could not find
    // specified service") when the label is not loaded, which IS that state,
    // and refusing it stopped the proxy step dead on 2026-09-03 -- both
    // candidates healthy on their candidate ports, nothing serving either
    // stable bind, and the control plane 503 behind that. 3 and 5 were
    // already tolerated for exactly this reason; 113 belongs with them.
    if status.success() || matches!(status.code(), Some(3) | Some(5) | Some(113)) {
        Ok(())
    } else {
        Err(format!(
            "legacy launchd service {label} bootout exited with {status}"
        ))
    }
}

/// Load the declared legacy unit if it is absent. Callers verify the stable
/// endpoint afterwards; a launchctl exit code cannot establish port ownership.
pub(crate) fn restore_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(plist) = target.legacy_launchd_plist.as_deref() else {
        return Ok(());
    };
    let label = target
        .legacy_launchd_label
        .as_deref()
        .ok_or_else(|| format!("legacy launchd plist {plist} has no declared service label"))?;
    let service = format!("system/{label}");
    let loaded = legacy_loaded(&service)?;
    let enabled = Command::new("/usr/bin/sudo")
        .args(["-n", "/bin/launchctl", "enable", &service])
        .output()
        .map_err(|error| format!("cannot enable legacy launchd service {service}: {error}"))?;
    if !enabled.status.success() {
        return Err(format!(
            "legacy launchd service {service} enable exited with {}: {}",
            enabled.status,
            String::from_utf8_lossy(&enabled.stderr).trim()
        ));
    }
    if loaded {
        return Ok(());
    }
    let bootstrapped = Command::new("/usr/bin/sudo")
        .args(["-n", "/bin/launchctl", "bootstrap", "system", plist])
        .output()
        .map_err(|error| format!("cannot restore legacy launchd service {plist}: {error}"))?;
    // A concurrent owner can load the same label between inspection and
    // bootstrap. Re-read the native state instead of guessing what exit 5 means.
    if legacy_loaded(&service)? {
        return Ok(());
    }
    Err(format!(
        "legacy launchd service {service} is not loaded after bootstrap of {plist} \
         ({}): {}",
        bootstrapped.status,
        String::from_utf8_lossy(&bootstrapped.stderr).trim()
    ))
}

fn legacy_loaded(service: &str) -> Result<bool, String> {
    let observed = Command::new("/usr/bin/sudo")
        .args(["-n", "/bin/launchctl", "print", service])
        .output()
        .map_err(|error| format!("cannot inspect legacy launchd service {service}: {error}"))?;
    if observed.status.success() {
        return Ok(true);
    }
    // launchctl's observed missing-service status, also used by stop_legacy.
    if observed.status.code() == Some(113) {
        return Ok(false);
    }
    Err(format!(
        "cannot inspect legacy launchd service {service} ({}): {}",
        observed.status,
        String::from_utf8_lossy(&observed.stderr).trim()
    ))
}

//! The declared legacy unit, booted out on the way into the stable bind and
//! bootstrapped back on the way out of it.

use std::process::Command;

use crate::release_control::ReleaseTargetPolicy;

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

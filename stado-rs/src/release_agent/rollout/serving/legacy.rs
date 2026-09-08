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

/// Hand the stable bind back to the declared legacy unit.
///
/// `enable` first: the unit was disabled the moment the release path took the
/// bind over, and `launchctl bootstrap` refuses a disabled service with exit 5
/// (`Bootstrap failed: 5: Input/output error`). Until 2026-09-06 that exit was
/// accepted as success here, so a rollback with no previous release recorded
/// "legacy restored" while nothing was bootstrapped: on charless-mac-mini the
/// skarbiec rollback killed its proxy, took exit 5 as the unit being back, and
/// the stable bind stayed empty for thirteen hours while the object API answered
/// `503 object authorization unavailable` to every host. A refusal is a refusal;
/// the caller decides what to do about the bind it still does not have.
pub(crate) fn restore_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(plist) = target.legacy_launchd_plist.as_deref() else {
        return Ok(());
    };
    if let Some(label) = target.legacy_launchd_label.as_deref() {
        let _ = Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/launchctl", "enable", &format!("system/{label}")])
            .status();
    }
    let status = Command::new("/usr/bin/sudo")
        .args(["-n", "/bin/launchctl", "bootstrap", "system", plist])
        .status()
        .map_err(|error| format!("cannot restore legacy launchd service {plist}: {error}"))?;
    if status.success() {
        Ok(())
    } else if status.code() == Some(5) {
        Err(format!(
            "legacy launchd service bootstrap of {plist} was refused (exit 5: the service is disabled or its plist is unloadable); the stable bind has no owner"
        ))
    } else {
        Err(format!(
            "legacy launchd service bootstrap exited with {status}"
        ))
    }
}

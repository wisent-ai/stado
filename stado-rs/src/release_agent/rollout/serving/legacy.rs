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
    let output = crate::wait::child_output(child, format!("legacy ownership read for {label}"))
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
    let Some(job_pid) = report
        .launchd_pid
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
    else {
        return Ok(false);
    };
    if report.unit != label
        || report.unit_path != plist
        || report.loaded != "yes"
        || report.listeners_state != service_serving::LISTENERS_READ
        || report.ports.len() != 1
        || report.ports[0].port != port
        || report.ports[0].holders.is_empty()
        || report.ports[0].holders.iter().any(|holder| {
            holder.owner_state != service_serving::OWNER_RESOLVED || holder.owner != label
        })
    {
        return Ok(false);
    }
    // Labels may exist in both system and user domains. Bind listeners to the
    // PID read from this exact system job, not merely to its label spelling.
    if report.ports[0]
        .holders
        .iter()
        .all(|holder| holder.pid.parse() == Ok(job_pid))
    {
        return Ok(true);
    }
    let output = crate::wait::output(Command::new("/bin/ps").args(["-axo", "pid=,ppid="]))
        .map_err(|error| format!("cannot read legacy listener ancestry for {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "legacy listener ancestry read for {label} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let parents: std::collections::BTreeMap<u32, u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    Ok(report.ports[0].holders.iter().all(|holder| {
        let Ok(mut pid) = holder.pid.parse::<u32>() else {
            return false;
        };
        for _ in 0..=parents.len() {
            if pid == job_pid {
                return true;
            }
            let Some(parent) = parents.get(&pid) else {
                return false;
            };
            if *parent == pid {
                return false;
            }
            pid = *parent;
        }
        false
    }))
}

/// The TCP ports the legacy job's own process listens on, read from launchd's
/// pid for the label (system domain, then this login's) and `lsof` for that
/// pid. Empty when the job is not running: a stopped unit serves nothing that
/// its bootout could take away.
fn legacy_listening_ports(label: &str) -> Result<Vec<u16>, String> {
    let uid = nix::unistd::getuid();
    let mut pid = None;
    for domain in [format!("system/{label}"), format!("gui/{uid}/{label}")] {
        let printed = crate::wait::output(Command::new("/bin/launchctl").args(["print", &domain]))
            .map_err(|error| format!("cannot read legacy launchd service {domain}: {error}"))?;
        pid = String::from_utf8_lossy(&printed.stdout)
            .lines()
            .find_map(|line| line.trim().strip_prefix("pid = "))
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        if pid.is_some() {
            break;
        }
    }
    let Some(pid) = pid else {
        return Ok(Vec::new());
    };
    let listening = crate::wait::output(Command::new("/usr/sbin/lsof").args([
        "-nP",
        "-a",
        "-p",
        &pid.to_string(),
        "-iTCP",
        "-sTCP:LISTEN",
        "-Fn",
    ]))
    .map_err(|error| format!("cannot read the listeners of legacy {label} (pid {pid}): {error}"))?;
    let mut ports: Vec<u16> = String::from_utf8_lossy(&listening.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter_map(|address| address.rsplit_once(':'))
        .filter_map(|(_, port)| port.parse::<u16>().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

pub(crate) fn stop_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(label) = target.legacy_launchd_label.as_deref() else {
        return Ok(());
    };
    // The release takes over the stable bind and nothing else. A legacy unit
    // that also answers another port is the only thing serving it, and
    // consumers declared on that port lose the service the moment the unit
    // goes, while the release reports itself healthy. Refuse, naming the
    // ports, so the endpoint is moved to the stable bind first.
    if let Ok(serving) = target.blue_green_serving() {
        let stable = serving
            .stable_bind
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok());
        let other: Vec<String> = legacy_listening_ports(label)?
            .into_iter()
            .filter(|port| Some(*port) != stable)
            .map(|port| port.to_string())
            .collect();
        if !other.is_empty() {
            return Err(format!(
                "legacy launchd service {label} also listens on port(s) {} that the release does not take over (it serves {}); \
                 point the consumers' service endpoint at the stable bind before the legacy unit is stopped",
                other.join(", "),
                serving.stable_bind
            ));
        }
    }
    let status = crate::wait::status(Command::new("/usr/bin/sudo").args([
        "-n",
        "/bin/launchctl",
        "bootout",
        &format!("system/{label}"),
    ]))
    .map_err(|error| format!("cannot disable legacy launchd service {label}: {error}"))?;
    // This asks for a state, not an action: the legacy unit must not hold the
    // port before the proxy binds it. launchd answers 113 ("Could not find
    // specified service") when the label is not loaded, which IS that state,
    // and refusing it stops the proxy step dead -- both
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

/// Load the declared legacy unit if it is absent, answering whether this call
/// loaded it. Callers verify the stable endpoint afterwards; a launchctl exit
/// code cannot establish port ownership.
pub(crate) fn restore_legacy(target: &ReleaseTargetPolicy) -> Result<bool, String> {
    let Some(plist) = target.legacy_launchd_plist.as_deref() else {
        return Ok(false);
    };
    let label = target
        .legacy_launchd_label
        .as_deref()
        .ok_or_else(|| format!("legacy launchd plist {plist} has no declared service label"))?;
    let service = format!("system/{label}");
    let loaded = legacy_loaded(&service)?;
    let enabled = crate::wait::output(Command::new("/usr/bin/sudo").args([
        "-n",
        "/bin/launchctl",
        "enable",
        &service,
    ]))
    .map_err(|error| format!("cannot enable legacy launchd service {service}: {error}"))?;
    if !enabled.status.success() {
        return Err(format!(
            "legacy launchd service {service} enable exited with {}: {}",
            enabled.status,
            String::from_utf8_lossy(&enabled.stderr).trim()
        ));
    }
    if loaded {
        return Ok(false);
    }
    let bootstrapped = crate::wait::output(Command::new("/usr/bin/sudo").args([
        "-n",
        "/bin/launchctl",
        "bootstrap",
        "system",
        plist,
    ]))
    .map_err(|error| format!("cannot restore legacy launchd service {plist}: {error}"))?;
    // A concurrent owner can load the same label between inspection and
    // bootstrap. Re-read the native state instead of guessing what exit 5 means.
    if legacy_loaded(&service)? {
        return Ok(true);
    }
    Err(format!(
        "legacy launchd service {service} is not loaded after bootstrap of {plist} \
         ({}): {}",
        bootstrapped.status,
        String::from_utf8_lossy(&bootstrapped.stderr).trim()
    ))
}

fn legacy_loaded(service: &str) -> Result<bool, String> {
    let observed = crate::wait::output(Command::new("/usr/bin/sudo").args([
        "-n",
        "/bin/launchctl",
        "print",
        service,
    ]))
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

// Vast.ai coexistence + auto-registration helper.
//
// Coexistence: when a paid Vast rental is running on this host, we want
// Wisent jobs to yield politely (no new Wisent container on top of the
// rental). Detected by container name (Vast convention: starts with "C.")
// or image prefix.
//
// Auto-registration: on agent startup, if VAST_HOST_API_KEY is present and
// the vastai daemon is not already active, we run the embedded install
// script once. This is what makes idle GPU time automatically rentable:
// every Wisent compute host also advertises on Vast while not busy.

use bollard::Docker;
use bollard::container::ListContainersOptions;

/// Heuristic: true if any running container on this host looks like a
/// Vast.ai rental. Called before accepting a new CREATE_CONTAINER to
/// decide whether to yield.
pub async fn is_vast_renting(docker: &Docker) -> bool {
    active_vast_container_count(docker).await > 0
}

/// Number of active Vast containers. Useful for logging and for future
/// policy like "allow up to N concurrent Vast rentals before yielding
/// all Wisent jobs".
pub async fn active_vast_container_count(docker: &Docker) -> usize {
    let opts = ListContainersOptions::<String> {
        all: false, // running only
        ..Default::default()
    };
    let containers = match docker.list_containers(Some(opts)).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("vast: list_containers failed: {e}");
            return 0;
        }
    };

    let mut count = 0usize;
    for c in &containers {
        let is_vast_name = c.names.as_deref().map(|ns| {
            ns.iter().any(|n| {
                let trimmed = n.trim_start_matches('/');
                trimmed.starts_with("C.") || trimmed.starts_with("vast-")
            })
        }).unwrap_or(false);

        let is_vast_image = c.image.as_deref()
            .map(|img| img.contains("vastai/") || img.contains("vast.ai/"))
            .unwrap_or(false);

        if is_vast_name || is_vast_image {
            count += 1;
        }
    }
    if count > 0 {
        tracing::info!("vast: {count} active rental container(s) detected; yielding Wisent jobs");
    }
    count
}

const VAST_INSTALL_SH: &str = include_str!("../vast_install.sh");

/// Register this host on Vast.ai so idle GPU time auto-rents. No-ops if
/// VAST_HOST_API_KEY is unset, if the daemon is already active, or if the
/// required system binaries are missing.
pub async fn ensure_host_daemon_installed() {
    let active = tokio::process::Command::new("systemctl")
        .args(["is-active", "vastai"])
        .output().await;
    if let Ok(out) = active {
        if out.status.success() {
            tracing::info!("vast: vastai daemon already active");
            return;
        }
    }

    let api_key = match std::env::var("VAST_HOST_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            tracing::info!("vast: VAST_HOST_API_KEY unset; host will not auto-rent");
            return;
        }
    };

    let script_path = "/tmp/wisent_vast_install.sh";
    if let Err(e) = tokio::fs::write(script_path, VAST_INSTALL_SH).await {
        tracing::error!("vast: could not stage install script: {e}");
        return;
    }

    tracing::info!("vast: installing host daemon");
    let out = tokio::process::Command::new("sudo")
        .args(["-E", "bash", script_path])
        .env("VAST_HOST_API_KEY", &api_key)
        .output().await;
    match out {
        Ok(o) if o.status.success() => tracing::info!("vast: install complete; host now listed"),
        Ok(o) => tracing::error!("vast: install exited {:?}: {}", o.status.code(),
            String::from_utf8_lossy(&o.stderr)),
        Err(e) => tracing::error!("vast: install spawn failed: {e}"),
    }
}

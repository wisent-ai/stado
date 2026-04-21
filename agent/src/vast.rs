// Vast.ai coexistence helper.
//
// When the host machine is registered on Vast.ai (vast.ai runs its own
// docker daemon containers alongside ours), we want Wisent jobs to yield
// politely: don't start a new Wisent container on top of an active paid
// Vast rental. Vast-labeled containers can be detected by container name
// (Vast convention: starts with "C.") or by image prefix.
//
// This module only inspects the local docker socket; it never talks to
// the Vast API. That keeps the agent free of Vast credentials.

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

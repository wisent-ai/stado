//! Retiring the proxy route a state file owns, whatever its bind.

use super::Owner;

/// Stop the route `state` owns, whatever its bind.
pub(super) async fn retire(state: &std::path::Path, owner: &Owner) -> Result<(), String> {
    if !state.is_absolute() {
        return Err("proxy retirement requires an absolute state path".to_string());
    }
    let Some(route) = owner.routes.lock().await.remove(state) else {
        return Ok(());
    };
    let bind = route.identity.bind;
    // The stop signal carries nothing back: a refused send only says the
    // forwarding loop had already ended.
    route
        .stop
        .send(())
        .map_err(|()| format!("proxy {bind} stopped before acknowledging retirement"))?;
    route
        .task
        .await
        .map_err(|error| format!("retiring proxy {bind} failed: {error}"))?;
    eprintln!(
        "stado release proxy retired: pid={} bind={bind} state={}",
        std::process::id(),
        state.display()
    );
    Ok(())
}

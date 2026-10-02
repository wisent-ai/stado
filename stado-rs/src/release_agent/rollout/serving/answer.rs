//! Prove the stable bind answers from the exact staged release, and put the
//! proxy in front of it when nothing already stands there.

use std::path::Path;

use super::discover::{exact_proxy_pid, pid_alive, proxy_process_matches};
use super::legacy::stop_legacy;
use super::proxy::{start_proxy, write_proxy_target, ProxyState};
use crate::release_agent::rollout::candidate::spawn::{readiness, Readiness};
use crate::release_agent::rollout::candidate::stage::marker_path;
use crate::release_agent::state::document::{proxy_state_path, save_state};
use crate::release_agent::state::records::{HostReleaseState, ProcessRecord};
use crate::release_cause::Refusal;
use crate::release_control::{
    self, BlueGreenServing, ReleaseManifest, ReleaseTargetPolicy, RolloutStrategy,
};

/// Prove that the exact live Stado proxy routes the exact staged release and
/// that the product accepts its declared readiness request on the stable bind.
///
/// Release identity does not belong to the product's readiness document. The
/// signed manifest and immutable archive establish the candidate's version and
/// digest before it starts; the process record and proxy target then bind that
/// identity to one candidate port. Requiring an undeclared `build.version`
/// field here made otherwise-valid readiness contracts impossible to satisfy.
async fn stable_bind_answer(
    proxy_pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    active: &ProcessRecord,
) -> Result<(), String> {
    if !pid_alive(proxy_pid) {
        return Err(format!("stable release proxy pid {proxy_pid} is gone"));
    }
    let marker = std::fs::read(marker_path(Path::new(&active.release_dir))).map_err(|error| {
        format!(
            "cannot read active release identity {}: {error}",
            active.release_dir
        )
    })?;
    let manifest: ReleaseManifest = serde_json::from_slice(&marker)
        .map_err(|error| format!("active release identity is invalid: {error}"))?;
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(&manifest)?);
    if manifest_sha != active.manifest_sha256
        || manifest.version != active.version
        || manifest.artifact_sha256 != active.artifact_sha256
    {
        return Err("active process does not match its immutable release identity".to_string());
    }

    let proxy_path = proxy_state_path(target, product);
    let proxy: ProxyState =
        serde_json::from_slice(&std::fs::read(&proxy_path).map_err(|error| {
            format!("cannot read proxy target {}: {error}", proxy_path.display())
        })?)
        .map_err(|error| format!("invalid proxy target {}: {error}", proxy_path.display()))?;
    let expected_upstream = format!("127.0.0.1:{}", active.port);
    if proxy.generation != generation || proxy.upstream != expected_upstream {
        return Err(format!(
            "stable proxy target is generation {} upstream {}, expected generation {generation} upstream {expected_upstream}",
            proxy.generation, proxy.upstream
        ));
    }

    // The candidate has already answered its readiness request, the proxy owns
    // the stable listener, and the proxy reads its target on every accepted
    // connection. One request therefore decides: a refusal here is a real
    // routing fault, reported as it happened.
    let url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    match reqwest::Client::new().get(&url).send().await {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(format!("{url} answered HTTP {}", response.status())),
        Err(error) => Err(format!("{url} did not answer: {error:#}")),
    }
}

/// Route the stable bind to `active` and prove it answers there.
///
/// `Ok(None)` is a release that answers through the stable bind.
/// `Ok(Some(why))` is a live release that refused this pass's readiness read
/// while the policy's readiness window, counted from its first refusal in
/// `state.readiness_lost_at`, is still open: the caller leaves everything as
/// it is and a later pass reads it again. `Err` is a release that is gone or
/// still refuses once the window is over, or a routing fault.
pub(crate) async fn ensure_active_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    active: &ProcessRecord,
    state: &mut HostReleaseState,
    strategy: &RolloutStrategy,
) -> Result<Option<Refusal>, Refusal> {
    // The probe's own sentence travels with the verdict. A quarantine list
    // reading `active release lost readiness` for two digests in a row says
    // nothing about whether the candidate answered 503 or refused the
    // connection. Two different repairs, one word.
    let lost_for = state
        .readiness_lost_at
        .map(|lost| {
            chrono::Utc::now()
                .signed_duration_since(lost)
                .num_seconds()
                .max(0) as u64
        })
        .unwrap_or_default();
    match readiness(active, &serving.readiness_path, strategy, lost_for).await {
        Readiness::Ready => {
            // A refusal that has been answered since must not start the next
            // refusal's window early.
            if state.readiness_lost_at.take().is_some() {
                save_state(target, state)?;
            }
        }
        Readiness::Pending(why) => {
            state.readiness_lost_at.get_or_insert_with(chrono::Utc::now);
            return Ok(Some(why));
        }
        Readiness::Failed(why) => {
            state.readiness_lost_at = None;
            return Err(why.context(|said| format!("active release lost readiness: {said}")));
        }
    }
    // A legacy unit can be loaded again after cutover while the stable proxy
    // remains healthy. Reassert release ownership on every reconcile, not only
    // when the proxy first starts; otherwise the legacy launcher can rewrite
    // shared runtime trust before failing to bind the already-owned port.
    stop_legacy(target)?;
    write_proxy_target(target, product, generation, active.port)?;

    let recorded_proxy = match state.proxy_pid {
        Some(proxy_pid) if proxy_process_matches(proxy_pid, target, serving, product).await? => {
            Some(proxy_pid)
        }
        _ => None,
    };
    let proxy_pid = if let Some(proxy_pid) = recorded_proxy {
        proxy_pid
    } else if let Some(proxy_pid) = exact_proxy_pid(target, serving, product).await? {
        proxy_pid
    } else {
        stop_legacy(target)?;
        let owner_pid = start_proxy(target, serving, product, generation, active.port).await?;
        let proxy_pid = exact_proxy_pid(target, serving, product)
            .await
            .map_err(|why| format!("stable release proxy failed to start: {why}"))?
            .ok_or_else(|| "host did not retain the requested stable proxy listener".to_string())?;
        if proxy_pid != owner_pid {
            return Err(format!(
                "stable release proxy acknowledged owner pid {owner_pid}, but exact owner is pid {proxy_pid}"
            )
            .into());
        }
        proxy_pid
    };

    state.proxy_pid = Some(proxy_pid);
    stable_bind_answer(proxy_pid, target, serving, product, generation, active)
        .await
        .map(|()| None)
        .map_err(|why| format!("stable release proxy is invalid: {why}").into())
}

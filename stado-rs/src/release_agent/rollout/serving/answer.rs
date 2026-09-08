//! Prove the stable bind answers from the exact staged release, and put the
//! proxy in front of it when nothing already stands there.

use std::path::Path;
use std::time::Duration;

use super::discover::{exact_proxy_pid, pid_alive, proxy_process_matches};
use super::legacy::stop_legacy;
use super::proxy::{start_proxy, write_proxy_target, ProxyState};
use crate::release_agent::rollout::candidate::spawn::not_ready_because;
use crate::release_agent::rollout::candidate::stage::marker_path;
use crate::release_agent::state::document::proxy_state_path;
use crate::release_agent::state::records::{HostReleaseState, ProcessRecord};
use crate::release_control::{self, BlueGreenServing, ReleaseManifest, ReleaseTargetPolicy};

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
    readiness_timeout_seconds: u64,
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

    let url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(readiness_timeout_seconds);
    let mut last_error = None;
    loop {
        if !pid_alive(proxy_pid) {
            return Err(format!("stable release proxy pid {proxy_pid} is gone"));
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "{url} did not become ready within {readiness_timeout_seconds}s; {}",
                last_error
                    .as_deref()
                    .unwrap_or("no response before the deadline")
            ));
        }
        last_error = Some(
            match client
                .get(&url)
                .timeout(remaining.min(Duration::from_secs(3)))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => format!("HTTP {}", response.status()),
                Err(error) => format!("{error:#}"),
            },
        );
        tokio::time::sleep(
            deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_millis(200)),
        )
        .await;
    }
}

pub(crate) async fn ensure_active_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    active: &ProcessRecord,
    state: &mut HostReleaseState,
    readiness_timeout_seconds: u64,
) -> Result<(), String> {
    // The probe's own sentence travels with the verdict. On 2026-09-06 the
    // quarantine list on charless-mac-mini read `active release lost readiness`
    // for two digests in a row, and nothing said whether the candidate answered
    // 503, refused the connection, or took longer than the 3s the probe allows
    // on a host running 242 jobs. Three different repairs, one word.
    if let Some(why) = not_ready_because(active, &serving.readiness_path).await {
        return Err(format!("active release lost readiness: {why}"));
    }
    // A legacy unit can be loaded again after cutover while the stable proxy
    // remains healthy. Reassert release ownership on every reconcile, not only
    // when the proxy first starts; otherwise the legacy launcher can rewrite
    // shared runtime trust before failing to bind the already-owned port.
    stop_legacy(target)?;
    write_proxy_target(target, product, generation, active.port)?;

    let recorded_proxy = match state.proxy_pid {
        Some(proxy_pid) if proxy_process_matches(proxy_pid, target, serving, product)? => {
            Some(proxy_pid)
        }
        _ => None,
    };
    let proxy_pid = if let Some(proxy_pid) = recorded_proxy {
        proxy_pid
    } else if let Some(proxy_pid) = exact_proxy_pid(target, serving, product)? {
        proxy_pid
    } else {
        stop_legacy(target)?;
        let spawned_pid = start_proxy(target, serving, product, generation, active.port)?;
        let proxy_pid = exact_proxy_pid(target, serving, product)
            .map_err(|why| format!("stable release proxy failed to start: {why}"))?
            .ok_or_else(|| "spawned stable release proxy is not live".to_string())?;
        if proxy_pid != spawned_pid {
            return Err(format!(
                "spawned stable release proxy pid {spawned_pid}, but exact owner is pid {proxy_pid}"
            ));
        }
        proxy_pid
    };

    state.proxy_pid = Some(proxy_pid);
    stable_bind_answer(
        proxy_pid,
        target,
        serving,
        product,
        generation,
        active,
        readiness_timeout_seconds,
    )
    .await
    .map_err(|why| format!("stable release proxy is invalid: {why}"))
}

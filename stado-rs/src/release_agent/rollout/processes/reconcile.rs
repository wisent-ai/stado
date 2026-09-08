//! The stable bind's own repair pass, run before any rollout branch can
//! return.

use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use super::inventory::{listener_pid, release_processes};
use crate::release_agent::rollout::serving::discover::{exact_proxy_pid, pid_alive};
use crate::release_agent::rollout::serving::legacy::{restore_legacy, stop_legacy};
use crate::release_agent::rollout::serving::proxy::{proxy_upstream_port, stable_bind_ready};
use crate::release_agent::state::document::save_state;
use crate::release_agent::state::records::HostReleaseState;
use crate::release_control::ReleaseTargetPolicy;

/// Reconcile a proxy that survived an agent handoff before its pid reached the
/// host state document.
///
/// The exact proxy argv belongs to this product and runs under the same
/// per-product reconcile lock as the state update. A live upstream is the crash
/// window: adopt the proxy rather than interrupting traffic. With no owned
/// release process and a dead upstream, the proxy can only pin the stable bind
/// to nowhere; retire it and let the declared legacy service reclaim the bind.
pub(crate) async fn reconcile_stable_proxy(
    target: &ReleaseTargetPolicy,
    product: &str,
    install_root: &str,
    readiness_timeout_seconds: u64,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    let serving = target.blue_green_serving()?;
    let ownership_empty =
        state.active.is_none() && state.candidate.is_none() && state.previous.is_none();
    let Some(proxy_pid) = exact_proxy_pid(target, &serving, product)? else {
        // No proxy, no owned release, and nothing answering on the stable bind:
        // the release path has let go of the bind and the legacy unit was never
        // given it back. This is the state a rollback whose `restore_legacy`
        // was refused leaves behind, and until 2026-09-06 the agent returned
        // here every fifteen seconds while charless-mac-mini served no
        // Skarbiec for thirteen hours. The bind belongs to someone on every
        // tick; when the release path does not want it, the declared unit does.
        if ownership_empty
            && target.legacy_launchd_plist.is_some()
            && !stable_bind_ready(&serving).await
        {
            restore_legacy(target)?;
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(readiness_timeout_seconds);
            while !stable_bind_ready(&serving).await {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "legacy {product} did not reclaim {} after the release path released it",
                        serving.stable_bind
                    ));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            eprintln!(
                "restored legacy {product} on {}: no release proxy and no owned release held the bind",
                serving.stable_bind
            );
        }
        return Ok(());
    };
    let upstream = proxy_upstream_port(target, product);
    let processes = release_processes(install_root);
    // Owned means a release process holds the upstream port. The argument
    // vector is asked first and the kernel second, because a launcher that was
    // not told `--port` -- skarbiec's is not -- answers `None` to the first
    // question and the proxy it serves would otherwise be retired as orphaned.
    let upstream_is_owned = upstream.is_some_and(|upstream_port| {
        processes
            .iter()
            .any(|process| process.port == Some(upstream_port))
            || listener_pid(upstream).is_some_and(|pid| {
                processes
                    .iter()
                    .any(|process| process.pid == pid || process.process_group == pid)
            })
    });
    // A surviving healthy proxy means the release path owns the stable bind.
    // Reconcile the declared legacy unit before adopting that proxy so a later
    // launchd reload cannot run both deployment paths at once.
    if upstream_is_owned {
        stop_legacy(target)?;
    }
    if upstream_is_owned && stable_bind_ready(&serving).await {
        state.proxy_pid = Some(proxy_pid);
        save_state(target, state)?;
        eprintln!(
            "adopted {product} release proxy pid={proxy_pid} after interrupted handoff; \
             upstream {upstream:?} is ready"
        );
        return Ok(());
    }

    if !ownership_empty {
        return Ok(());
    }
    let _ = kill(Pid::from_raw(proxy_pid), Signal::SIGTERM);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while pid_alive(proxy_pid) {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "orphaned {product} release proxy pid={proxy_pid} did not exit"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    restore_legacy(target)?;
    if target.legacy_launchd_plist.is_some() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(readiness_timeout_seconds);
        while !stable_bind_ready(&serving).await {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "legacy {product} did not reclaim {} after orphaned proxy retirement",
                    serving.stable_bind
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    state.proxy_pid = None;
    save_state(target, state)?;
    eprintln!(
        "retired orphaned {product} release proxy pid={proxy_pid}; upstream \
         {upstream:?} had no ready owned release"
    );
    Ok(())
}

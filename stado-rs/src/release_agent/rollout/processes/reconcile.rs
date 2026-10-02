//! The stable bind's own repair pass, run before any rollout branch can
//! return.

use super::inventory::{listener_pid, release_processes};
use crate::release_agent::rollout::serving::discover::exact_proxy_pid;
use crate::release_agent::rollout::serving::legacy::{restore_legacy, stop_legacy};
use crate::release_agent::rollout::serving::proxy::{proxy_upstream_port, stable_bind_ready};
use crate::release_agent::state::document::save_state;
use crate::release_agent::state::records::HostReleaseState;
use crate::release_control::ReleaseTargetPolicy;

/// Reconcile a listener retained by the host when a finite release command
/// ended before saving its owner pid in the host state document.
///
/// The authenticated host reports the exact state path and bind. This repair
/// runs under the same per-product reconcile lock as the state update. Adopt
/// a listener with a ready owned upstream; otherwise remove that listener,
/// never signal the shared host process, and restore the declared legacy unit.
///
/// `leave_bind_for_candidate` is the caller's answer to "is a candidate about
/// to be spent on this bind, having never had it". It exists because this pass
/// runs before every rollout branch: with the legacy unit holding the stable
/// port, the agent refuses to spawn a candidate for it, and when the unit is
/// stopped this pass would put the same unit straight back seconds later. Two
/// correct rules, one loop nothing can leave, and behind it every credential
/// write on that host.
pub(crate) async fn reconcile_stable_proxy(
    target: &ReleaseTargetPolicy,
    product: &str,
    install_root: &str,
    leave_bind_for_candidate: bool,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    let serving = target.blue_green_serving()?;
    let ownership_empty =
        state.active.is_none() && state.candidate.is_none() && state.previous.is_none();
    let Some(proxy_pid) = exact_proxy_pid(target, &serving, product).await? else {
        // No proxy, no owned release, and nothing answering on the stable bind:
        // the release path has let go of the bind and the legacy unit was never
        // given it back. This is the state a rollback whose `restore_legacy`
        // was refused leaves behind, and returning here every fifteen seconds
        // would leave the host serving no Skarbiec indefinitely. The bind
        // belongs to someone on every tick; when the release path does not
        // want it, the declared unit does.
        if ownership_empty
            && target.legacy_launchd_plist.is_some()
            && !stable_bind_ready(&serving).await
        {
            // The safety net stands except for the one tick a candidate is
            // owed: restoring the legacy unit here would take the bind back
            // before the rollout could ask for it, and the next pass would
            // read the same held bind that sent it here. A candidate that
            // then fails quarantines its digest, so the following tick has
            // nothing to roll out and this net catches the bind again.
            if leave_bind_for_candidate {
                eprintln!(
                    "left {} free for this tick's {product} candidate: the declared unit is not \
                     restored while a release that has never held the bind is owed one",
                    serving.stable_bind
                );
                return Ok(());
            }
            // A unit this pass loads has not had a chance to answer yet; the
            // next pass reads the bind again. A unit that was already loaded
            // and still does not answer is broken, and that is said here.
            if !restore_legacy(target)? {
                return Err(format!(
                    "legacy {product} is loaded but does not answer on {}: no release proxy \
                     and no owned release holds the bind",
                    serving.stable_bind
                ));
            }
            eprintln!(
                "loaded legacy {product} to reclaim {}: no release proxy and no owned release \
                 held the bind; the next pass reads its answer",
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
    crate::release_agent::rollout::serving::control::stop(
        Some(&target.home),
        &crate::release_agent::state::document::proxy_state_path(target, product),
        &serving.stable_bind,
    )
    .await?;
    restore_legacy(target)?;
    state.proxy_pid = None;
    save_state(target, state)?;
    eprintln!(
        "retired orphaned {product} release proxy pid={proxy_pid}; upstream \
         {upstream:?} had no ready owned release; the next pass reads the legacy \
         unit's answer on {}",
        serving.stable_bind
    );
    Ok(())
}

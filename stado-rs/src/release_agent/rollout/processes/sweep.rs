//! The pass that retires release processes the state document does not name.

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use super::inventory::{listener_pid, release_processes, swept_before};
use crate::release_agent::rollout::serving::proxy::proxy_upstream_port;
use crate::release_agent::state::records::HostReleaseState;
use crate::release_control::ReleaseTargetPolicy;

/// Terminate release processes the state file does not know about.
///
/// The agent owned every one of them once; a rollout that died between spawn and
/// save leaves the process running and the record absent, and the agent trusted
/// only the record. On the always-on Mac that produced a candidate from three
/// releases ago holding a candidate port for hours, a rollout that could never
/// bind past it, and an operator asked to stop processes by hand -- which is not a
/// release system. The one process spared is whichever serves the proxy's current
/// upstream: it carries traffic, and the normal cutover retires it by routing away
/// first, after which the next pass sweeps it here.
///
/// A leak is a process this agent started. `spawn_release` marks every one of
/// its launches with `STADO_RELEASE_PRODUCT=` in the argument vector and puts
/// the launch in its own process group, so a leaked release is recognised by
/// that marker on itself or on its group leader. A process running the
/// release binary with neither -- another product's client, such as the Weles
/// capability broker running the attested Skarbiec binary -- is not this
/// agent's to stop; it is named in the log and left alone.
pub(crate) fn sweep_leaked_processes(
    target: &ReleaseTargetPolicy,
    product: &str,
    install_root: &str,
    state: &HostReleaseState,
) {
    let mut known = Vec::new();
    for record in [&state.active, &state.candidate, &state.previous]
        .into_iter()
        .flatten()
    {
        known.push(record.pid);
    }
    if let Some(pid) = state.proxy_pid {
        known.push(pid);
    }
    let upstream = proxy_upstream_port(target, product);
    let processes = release_processes(install_root);
    let spawned_groups: Vec<i32> = processes
        .iter()
        .filter(|process| process.agent_spawned)
        .map(|process| process.process_group)
        .collect();
    for process in &processes {
        if known.contains(&process.process_group) {
            continue;
        }
        if !process.agent_spawned && !spawned_groups.contains(&process.process_group) {
            eprintln!(
                "{product} {} pid={} runs the release binary without this agent's launch \
                 marker; it belongs to another product and is not swept",
                process.version, process.pid
            );
            continue;
        }
        if upstream.is_some()
            && (process.port == upstream || listener_pid(upstream) == Some(process.pid))
        {
            eprintln!(
                "leaked {product} {} pid={} still carries traffic on \
                 {:?}; retiring by cutover, not by kill",
                process.version, process.pid, upstream
            );
            continue;
        }
        // A process that ignored the previous pass's SIGTERM is still here on
        // this one. Sending it the same signal again is not a sweep, it is a log
        // line: skarbiec 0.2.39 pid 38640 on charless-mac-mini was "swept" every
        // fifteen seconds for thirteen hours on 2026-09-06 and never exited.
        let signal = if swept_before(target, product, process.pid) {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = kill(Pid::from_raw(process.pid), signal);
        eprintln!(
            "swept leaked {product} {} pid={} port={:?} signal={signal:?}",
            process.version, process.pid, process.port
        );
    }
}

//! What the kernel and the process table say: whether a pid still lives, and
//! whether it is the exact Stado proxy that owns this product's stable bind.

use std::path::Path;
use std::process::Command;

pub(crate) use super::process::{
    pid_alive, process_executable_matches, same_executable, terminate,
};
use crate::release_agent::state::document::proxy_state_path;
use crate::release_control::{BlueGreenServing, ReleaseTargetPolicy};

/// Bind the recorded PID to a live, authenticated host owner and its actual
/// state-file/listener pair. A healthy but unrelated listener grants nothing.
pub(crate) async fn proxy_process_matches(
    pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    Ok(exact_proxy_pid(target, serving, product).await? == Some(pid))
}

pub(crate) async fn exact_proxy_pid(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<i32>, String> {
    crate::release_agent::rollout::serving::control::inspect(
        Some(&target.home),
        &proxy_state_path(target, product),
        &serving.stable_bind,
    )
    .await
}

/// The `lsof` this host carries, or `None` when it carries none.
///
/// Both paths are real: macOS ships `/usr/sbin/lsof`, most Linux
/// distributions `/usr/bin/lsof`, and the host programs in
/// [`crate::deploy`] already probe exactly this pair in this order.
pub(crate) fn lsof_binary() -> Option<&'static Path> {
    ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .into_iter()
        .map(Path::new)
        .find(|candidate| candidate.is_file())
}

/// Report a stable listener unless native evidence identifies our proxy or
/// the explicitly declared system predecessor with separate candidate ports.
///
/// A missing or failing lsof retains the existing unknown result. Recognizing
/// a legacy predecessor requires the shared service ownership reader to prove
/// every listener's label; a matching executable name grants nothing.
pub(crate) async fn foreign_stable_bind_holder(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<String>, String> {
    let port = serving
        .stable_bind
        .rsplit(':')
        .next()
        .filter(|port| port.chars().all(|character| character.is_ascii_digit()))
        .ok_or_else(|| {
            format!(
                "{product} stable bind {} names no port",
                serving.stable_bind
            )
        })?;
    let port_number = port
        .parse::<u16>()
        .map_err(|error| format!("invalid stable bind {}: {error}", serving.stable_bind))?;
    if !serving.candidate_ports.contains(&port_number)
        && crate::release_agent::rollout::serving::legacy::owns_stable_bind(target, port_number)?
    {
        // The declared predecessor stays live until candidate readiness passes.
        // ensure_active_proxy owns the later stop, cutover and rollback path.
        return Ok(None);
    }
    let Some(lsof) = lsof_binary() else {
        return Ok(None);
    };
    let Ok(output) = Command::new(lsof)
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .output()
    else {
        return Ok(None);
    };
    let ours = exact_proxy_pid(target, serving, product).await?;
    let mut holder: Option<(i32, String)> = None;
    for field in String::from_utf8_lossy(&output.stdout).lines() {
        match field.split_at(1) {
            ("p", pid) => {
                let pid = pid.trim().parse::<i32>().unwrap_or_default();
                holder = (pid > 0 && Some(pid) != ours).then_some((pid, String::new()));
            }
            ("c", command) => {
                if let Some((_, name)) = holder.as_mut() {
                    *name = command.trim().to_string();
                }
            }
            _ => {}
        }
        if let Some((pid, name)) = holder.as_ref().filter(|(_, name)| !name.is_empty()) {
            return Ok(Some(describe_holder(
                &serving.stable_bind,
                *pid,
                name,
                product,
            )));
        }
    }
    Ok(holder.map(|(pid, _)| {
        format!(
            "{} is held by pid {pid}, which is not {product}'s release proxy",
            serving.stable_bind
        )
    }))
}

/// Describe a refused listener. A process name is diagnostic context, not
/// evidence that the declared lifecycle can replace it.
pub(crate) fn describe_holder(stable_bind: &str, pid: i32, name: &str, product: &str) -> String {
    format!(
        "{stable_bind} is held by pid {pid} ({name}), which is not {product}'s release proxy \
         or a proven declared legacy owner with independent candidate ports"
    )
}

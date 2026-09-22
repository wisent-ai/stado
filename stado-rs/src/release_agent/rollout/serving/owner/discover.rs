//! What the kernel and the process table say: whether a pid still lives, and
//! whether it is the exact Stado proxy that owns this product's stable bind.

use std::path::Path;
use std::process::Command;

pub(crate) use super::process::{
    pid_alive, process_executable_matches, same_executable, terminate,
};
use crate::release_agent::state::document::proxy_state_path;
use crate::release_control::{BlueGreenServing, ReleaseTargetPolicy};

/// The argument vector one Stado release proxy is recognised by, without its
/// program name: the program is compared as an executable, not as text.
fn proxy_arguments(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> String {
    format!(
        "release proxy --state {} --bind {}",
        proxy_state_path(target, product).display(),
        serving.stable_bind
    )
}

/// Split one `ps -o command=` line into the program it ran and the rest.
fn program_and_arguments(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let split = line.find(char::is_whitespace)?;
    Some((&line[..split], line[split..].trim_start()))
}

pub(crate) fn proxy_process_matches(
    pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    if !process_executable_matches(pid, &executable) {
        return Ok(false);
    }
    let expected_arguments = proxy_arguments(target, serving, product);
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy pid {pid}: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let observed = String::from_utf8_lossy(&output.stdout);
    Ok(
        program_and_arguments(&observed).is_some_and(|(program, arguments)| {
            arguments == expected_arguments && same_executable(Path::new(program), &executable)
        }),
    )
}

/// Find the live Stado proxy whose executable and complete argument vector own
/// this exact state file and bind.
///
/// A release proxy binds before entering its accept loop and exits when the
/// bind fails. After the caller's spawn grace period, a live exact match is
/// therefore the process that owns the bind, not merely another program that
/// happens to answer the product's readiness URL.
pub(crate) fn exact_proxy_pid(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<i32>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let expected_arguments = proxy_arguments(target, serving, product);
    let output = Command::new("/bin/ps")
        .args(["axww", "-o", "pid=", "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy processes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect stable proxy processes: /bin/ps exited {}",
            output.status
        ));
    }
    let mut matches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let split = line.find(char::is_whitespace)?;
            let pid = line[..split].parse::<i32>().ok()?;
            let (program, arguments) = program_and_arguments(&line[split..])?;
            (arguments == expected_arguments
                && same_executable(Path::new(program), &executable)
                && pid_alive(pid)
                && process_executable_matches(pid, &executable))
            .then_some(pid)
        })
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    match matches.as_slice() {
        [pid] => Ok(Some(*pid)),
        [] => Ok(None),
        many => Err(format!(
            "{} live Stado release proxies claim {} with state {}",
            many.len(),
            serving.stable_bind,
            proxy_state_path(target, product).display()
        )),
    }
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
pub(crate) fn foreign_stable_bind_holder(
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
    let ours = exact_proxy_pid(target, serving, product)?;
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

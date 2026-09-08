//! The `weles-api-runtime` workload: move one host's managed Weles API to a
//! named revision, or refuse and leave it where it was.

use crate::cli::CmdError;
use crate::deploy::host_channel;

const WELES_API_SERVICE: &str = "weles-api";
const WELES_API_PORT: u16 = 8788;
const WELES_API_WORK_DIR: &str = ".stado/build-work/weles-api-managed";
const WELES_SOURCE_REPOSITORY: &str = "https://github.com/wisent-ai/weles.git";
const WELES_API_BUILD_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

pub(crate) async fn refresh_weles_api_runtime(
    target: &str,
    revision: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CmdError::usage(
            "weles-api-runtime plan revision must be one full 40-character git object name",
        ));
    }
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let refused = |step: &str, detail: String| {
        CmdError::click(format!(
            "{}: the runtime was not moved to {revision}; {step} refused: {detail}",
            resolved.name
        ))
    };
    let home = host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let work = format!("{home}/{WELES_API_WORK_DIR}");
    let quoted_work = crate::deploy::shlex_quote(&work);
    let path = crate::deploy::shlex_quote(WELES_API_BUILD_PATH);
    let marker = format!("{work}/.weles-api-revision");
    let cloned = host_channel::remote_test(&resolved, &format!("-d {quoted_work}/.git"), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut steps: Vec<(&str, String)> = Vec::new();
    if !cloned {
        steps.push((
            "clone",
            format!(
                "PATH={path} git clone --filter=blob:none --no-checkout {} {quoted_work}",
                crate::deploy::shlex_quote(WELES_SOURCE_REPOSITORY)
            ),
        ));
    }
    steps.extend([
        (
            "fetch",
            format!("PATH={path} git -C {quoted_work} fetch origin {revision}"),
        ),
        (
            "checkout",
            format!("PATH={path} git -C {quoted_work} checkout --detach --force {revision}"),
        ),
        (
            "install",
            format!("cd {quoted_work} && PATH={path} npm ci --ignore-scripts"),
        ),
        (
            "node-pty helper",
            format!(
                "PATH={path} chmod u=rwx,go=rx {quoted_work}/node_modules/node-pty/prebuilds/*/spawn-helper \
                 {quoted_work}/node_modules/node-pty/build/Release/spawn-helper 2>/dev/null || true"
            ),
        ),
        (
            "recording dependency",
            format!("cd {quoted_work} && PATH={path} npx --no-install playwright install ffmpeg"),
        ),
        (
            "build",
            format!("cd {quoted_work} && PATH={path} npm run build"),
        ),
        (
            "record",
            format!(
                "PATH={path} printf '%s\\n' {} > {}",
                crate::deploy::shlex_quote(revision),
                crate::deploy::shlex_quote(&marker)
            ),
        ),
    ]);
    for (step, command) in steps {
        let output = host_channel::run_command(&resolved, &command, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !output.ok() {
            return Err(refused(
                step,
                host_channel::last_error_line(&output, "no output"),
            ));
        }
    }

    let recorded = host_channel::run_command(
        &resolved,
        &format!("cat {}", crate::deploy::shlex_quote(&marker)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let observed = recorded.stdout.trim();
    if observed != revision {
        return Err(refused(
            "readback",
            format!(
                "the host recorded {} in {marker}",
                if observed.is_empty() {
                    "nothing"
                } else {
                    observed
                }
            ),
        ));
    }

    let listeners = host_channel::run_command(
        &resolved,
        &format!("PATH={path} lsof -tiTCP:{WELES_API_PORT} -sTCP:LISTEN 2>/dev/null || true"),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let mut ended = Vec::new();
    for pid in listeners
        .stdout
        .split_whitespace()
        .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
    {
        let described = host_channel::run_command(
            &resolved,
            &format!("PATH={path} ps -p {pid} -o command= 2>/dev/null || true"),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        let command = described.stdout.trim();
        if command.is_empty() {
            continue;
        }
        if !command.contains("weles-api-server") && !command.contains("weles-api-launcher") {
            return Err(refused(
                "port takeover",
                format!(
                    "port {WELES_API_PORT} is held by pid {pid}, which is not a Weles API: {command}"
                ),
            ));
        }
        let stopped =
            host_channel::run_command(&resolved, &format!("PATH={path} kill -TERM {pid}"), &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        if !stopped.ok() {
            return Err(refused(
                "port takeover",
                format!(
                    "pid {pid} on port {WELES_API_PORT} refused SIGTERM: {}",
                    host_channel::last_error_line(&stopped, "no output")
                ),
            ));
        }
        ended.push(pid.to_string());
        if !json_output {
            println!(
                "{}: ended unowned Weles API pid {pid} on port {WELES_API_PORT}",
                resolved.name
            );
        }
    }
    if json_output && !ended.is_empty() {
        eprintln!(
            "{} ended unowned Weles API pid(s) {} before the managed restart",
            resolved.name,
            ended.join(", ")
        );
    }
    crate::cli::service::restart(
        WELES_API_SERVICE,
        Some(&resolved.name),
        None,
        None,
        json_output,
    )
    .await?;
    if !json_output {
        println!(
            "{}: {WELES_API_SERVICE} now serves {revision}",
            resolved.name
        );
    }
    Ok(())
}

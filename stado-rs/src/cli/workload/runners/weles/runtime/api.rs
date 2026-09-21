//! The `weles-api-runtime` workload: move one host's managed Weles API to a
//! named revision, or refuse and leave it where it was.

use crate::cli::CmdError;
use crate::deploy::host_channel;

const WELES_API_SERVICE: &str = "weles-api";
const WELES_API_PORT: u16 = 8788;
const WELES_API_WORK_DIR: &str = ".stado/build-work/weles-api-managed";
const WELES_SOURCE_REPOSITORY: &str = "https://github.com/wisent-ai/weles.git";
const WELES_API_BUILD_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
/// How long one health probe may hang before it is retried.
const PROBE_TIMEOUT_SECONDS: u64 = 5;
/// How long to wait between probes while the daemon starts.
const PROBE_INTERVAL_SECONDS: u64 = 3;
/// Enough attempts to cover a cold start; beyond it the unit is not starting.
const PROBE_ATTEMPTS: u64 = 20;
/// Where the unit's own stderr lands, and the diagnosis when it never serves.
const WELES_API_SERVICE_LOG: &str = "com.wisent.compute.service.weles-api.log";

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
        // A built checkout is not yet a servable tree: Weles refuses to start
        // without `release/source-identity.json` and its native runtime, and
        // both belong to a published release rather than to git. Weles owns
        // what a servable tree contains, so Weles puts them there.
        (
            "managed runtime",
            format!("cd {quoted_work} && PATH={path} node release/native/managed.mjs prepare"),
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
    // A restarted unit is not a served revision. launchd accepts a restart
    // for a program that dies on its first line, and this command used to
    // print "now serves <revision>" from that acceptance alone: on
    // 2026-09-21 it reported charless-mac-mini moved to 5588fb03 while the
    // daemon was in a crash loop printing `required Weles native runtime is
    // unavailable`, and only a separate activity read said the API was
    // silent. What the API answers is the proof, and its own log is the
    // diagnosis when it never answers.
    let served = wait_for_served_revision(&resolved, &runner, &path, revision).await?;
    if !json_output {
        println!(
            "{}: {WELES_API_SERVICE} answers on port {WELES_API_PORT} serving {served}",
            resolved.name
        );
    }
    Ok(())
}

/// Poll the API's own health answer until it names the revision, and refuse
/// with the unit's last words when it does not.
async fn wait_for_served_revision(
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    path: &str,
    revision: &str,
) -> Result<String, CmdError> {
    let probe = format!(
        "PATH={path} curl --silent --show-error --max-time {PROBE_TIMEOUT_SECONDS} \
         http://127.0.0.1:{WELES_API_PORT}/healthz"
    );
    let mut last = String::new();
    for attempt in 0..PROBE_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(PROBE_INTERVAL_SECONDS)).await;
        }
        let answer = host_channel::run_command(resolved, &probe, runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !answer.ok() {
            last = host_channel::last_error_line(&answer, "the health probe returned nothing");
            continue;
        }
        last = answer.stdout.trim().to_string();
        let Ok(document) = serde_json::from_str::<serde_json::Value>(&last) else {
            continue;
        };
        let served = document
            .get("sourceRevision")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if served == revision {
            return Ok(served.to_string());
        }
        if !served.is_empty() {
            last = format!("the API answers serving {served}, not {revision}");
        }
    }
    let log = host_channel::run_command(
        resolved,
        &format!(
            "PATH={path} tail -n 5 $HOME/.stado/logs/{WELES_API_SERVICE_LOG} 2>/dev/null || true"
        ),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let tail = log.stdout.trim();
    Err(CmdError::click(format!(
        "{}: {WELES_API_SERVICE} was restarted but never served {revision}: after {}s the health \
         probe on port {WELES_API_PORT} said {}; its log ends with: {}",
        resolved.name,
        PROBE_ATTEMPTS * PROBE_INTERVAL_SECONDS,
        if last.is_empty() { "nothing" } else { &last },
        if tail.is_empty() { "nothing" } else { tail },
    )))
}

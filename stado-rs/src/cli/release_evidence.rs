//! `stado release logs` and `stado release doctor` — the candidate's own
//! account of why a rollout stopped, and one verdict over every fact that
//! decides whether it can ever finish.
//!
//! NO Python original. Both exist because of one diagnosis that the shipped
//! commands could not finish. A brama candidate on `control-host` died
//! in under ninety seconds, and every fact the fleet published about it was
//! the outside view: the rollout state said
//! `candidate did not become ready within 90s: pid 46748 is gone` and
//! nothing else. The candidate's own stderr — the process explaining its own
//! exit — sat unread in `/Users/charles/.stado/logs/brama-0.2.27.err` on the
//! host, because nothing in the CLI reads that file. The operator guessed,
//! then read it by hand over ssh.
//!
//! So `release logs` fetches exactly that file, and `release doctor` answers
//! the question the operator actually asked before opening it — "will this
//! rollout ever land, and if not, what is holding it" — by joining the four
//! facts that were each individually visible and never once assembled:
//! desired versus observed release, the candidate's liveness and health, the
//! quarantine map (a desired digest sitting in it is a rollout that will
//! never retry), and the host's claiming gates (the same Mac mini stopped
//! claiming for hours on `disk_pressure_unresolved` with nothing in the CLI
//! saying so).
//!
//! Both are strictly read-only and safe against a live production host:
//! they read files and ask one loopback readiness URL for its status. They
//! start nothing, stop nothing, and write nothing — neither takes a
//! `--reason`, because neither changes any state to audit.
//!
//! The remote transport is not this module's: file reads go through
//! [`crate::cli::release_quarantine`]'s readers, which ride the one
//! registry ssh channel ([`crate::deploy::host_channel`]). The only remote
//! program here is the candidate probe below, and it is a compile-time
//! script with three quoted bindings.

use clap::{Args, ValueEnum};
use serde_json::{json, Value};

use super::release_quarantine::{
    canonical_control, compute_target, remote_read, remote_read_tail, resolve_target,
};
use super::CmdError;
use crate::deploy::{host_channel, host_gates, production_runner, shlex_quote};
use crate::release_agent::{
    self, host_log_path, release_status_uri, HostReleaseState, RolloutPhase,
};

/// The tail every operator wanted in the incident: enough to carry a panic
/// and its backtrace's first frames, short enough to read in a terminal.
const DEFAULT_LINES: usize = 40;

/// The log file was there and had bytes in it.
pub const STREAM_READ: &str = "read";
/// No such file on the host. Reported as its own word rather than as an
/// empty `lines` array: "the product wrote nothing" and "the product never
/// got far enough to have a log opened for it" send an operator to opposite
/// places, and the incident turned on exactly that distinction.
pub const STREAM_MISSING: &str = "missing";
/// The file exists and is zero bytes — the agent opened it, so the spawn
/// happened, and the product said nothing before it went.
pub const STREAM_EMPTY: &str = "empty";

/// The candidate answered its declared readiness path with a 2xx.
pub const HEALTH_OK: &str = "ok";
/// Nothing answered the candidate port at all.
pub const HEALTH_UNREACHABLE: &str = "unreachable";
/// The rollout state names no candidate, so there was nothing to probe.
/// Not [`HEALTH_UNREACHABLE`]: no candidate is a rollout that is not in
/// flight, and a dead candidate is a rollout that is failing.
pub const HEALTH_NO_CANDIDATE: &str = "no_candidate";
/// The target declares no readiness path (a `replace` strategy has nothing
/// HTTP to ask), so no probe was made.
pub const HEALTH_UNPROBED: &str = "unprobed";

/// No host has published a rollout status for this product and no state
/// file could be read, so the phase is unknown rather than idle.
pub const PHASE_UNREPORTED: &str = "unreported";

/// Observed release equals desired release and nothing is in flight.
pub const VERDICT_SETTLED: &str = "settled";
/// A candidate is staged or running, or observed still differs from
/// desired with nothing blocking the agent.
pub const VERDICT_ROLLING: &str = "rolling";
/// The rollout cannot proceed on its own. Both causes are silent today:
/// a quarantined desired digest is skipped forever, and an unresolved disk
/// gate stops the host from claiming anything at all.
pub const VERDICT_BLOCKED: &str = "blocked";

/// The desired artifact's digest is in the host's quarantine map. The agent
/// will refuse this exact release on every pass until the digest is cleared
/// (`stado release quarantine clear`) or a new version is promoted.
pub const BLOCKER_DESIRED_DIGEST_QUARANTINED: &str = "desired_digest_quarantined";
/// A candidate is recorded and is not answering its readiness path. Listed
/// as a blocker even while the verdict stays [`VERDICT_ROLLING`], because
/// the rollout is still inside its readiness window and the next thing that
/// happens to it is a quarantine.
pub const BLOCKER_CANDIDATE_NOT_READY: &str = "candidate_not_ready";

/// The marker the candidate probe prints, in the tab-delimited `STADO_*`
/// family every script on this channel speaks.
const CANDIDATE_MARKER: &str = "STADO_CANDIDATE";

/// The one remote program this module owns: is the recorded candidate pid
/// still there, and what does its readiness path answer.
///
/// `kill -0` is the shell builtin, not `/bin/kill`, so the probe forks
/// nothing to establish liveness — the fact the incident's state file
/// reported (`pid 46748 is gone`) and the fact it did not (whether the port
/// answers) come back from one round trip.
///
/// A curl failure is not an error here. "Nothing is listening" is an answer
/// to the operator's question, so the status word is empty and this side
/// reports [`HEALTH_UNREACHABLE`]; a script that exited non-zero would turn
/// the diagnosis into a transport failure.
const CANDIDATE_PROBE_BODY: &str = r#"set -u
LC_ALL=C
export LC_ALL
if kill -0 "$pid" 2>/dev/null; then
  alive=true
else
  alive=false
fi
status=$(/usr/bin/curl --silent --output /dev/null --max-time 3 \
  --write-out '%{http_code}' "http://127.0.0.1:$port$readiness_path" 2>/dev/null) || status=""
printf 'STADO_CANDIDATE\t%s\t%s\n' "$alive" "$status"
"#;

/// The probe bound to one recorded candidate.
///
/// `pid` and `port` are typed integers, so they reach the shell as digits.
/// The readiness path is the registry's own declaration and is quoted the
/// way [`crate::deploy::host_inventory::remote_inventory_script`] quotes its
/// declared program set — the same rule, so no operator input and no
/// unquoted registry value ever reaches the remote shell.
fn candidate_probe_script(pid: i32, port: u16, readiness_path: &str) -> String {
    format!(
        "pid={pid}\nport={port}\nreadiness_path={}\n{CANDIDATE_PROBE_BODY}",
        shlex_quote(readiness_path)
    )
}

/// Which of a candidate's two logs to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StreamArg {
    Out,
    Err,
    Both,
}

impl StreamArg {
    /// The file extensions [`crate::release_agent`] writes, in the order an
    /// operator reads them: stderr first. In the incident the answer was in
    /// `.err` and `.out` was empty, and printing stdout first buries it.
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Out => &["out"],
            Self::Err => &["err"],
            Self::Both => &["err", "out"],
        }
    }
}

#[derive(Args)]
pub struct ReleaseLogsArgs {
    pub product: String,
    /// Registry target whose host holds the logs.
    #[arg(long)]
    target: String,
    /// Release version to read logs for. Defaults to the desired version,
    /// which is the version any candidate on the host is running.
    #[arg(long)]
    version: Option<String>,
    #[arg(long, value_enum, default_value_t = StreamArg::Both)]
    stream: StreamArg,
    #[arg(long, default_value_t = DEFAULT_LINES)]
    lines: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseDoctorArgs {
    pub product: String,
    /// Registry target to diagnose. Optional when the product declares
    /// exactly one.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    json: bool,
}

/// One release log as the report carries it.
struct StreamReport {
    stream: &'static str,
    path: String,
    bytes: Option<u64>,
    lines: Vec<String>,
    state: &'static str,
}

impl StreamReport {
    fn to_value(&self) -> Value {
        json!({
            "stream": self.stream,
            "path": self.path,
            "bytes": self.bytes,
            "lines": self.lines,
            "state": self.state,
        })
    }
}

/// The word [`RolloutPhase`] publishes for itself, so the phase this command
/// prints is the phase the state file and the published status row spell.
fn phase_word(phase: RolloutPhase) -> String {
    serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| PHASE_UNREPORTED.to_string())
}

/// Is a candidate staged, started or under observation right now?
///
/// Enumerated rather than expressed as "not one of the terminal phases":
/// a phase added later must be classified deliberately, not inherit
/// "rolling" from a negation and make a stuck rollout read as one in flight.
fn phase_is_rolling(phase: RolloutPhase) -> bool {
    matches!(
        phase,
        RolloutPhase::Downloaded
            | RolloutPhase::Verified
            | RolloutPhase::Staged
            | RolloutPhase::CandidateRunning
            | RolloutPhase::Ready
            | RolloutPhase::Routed
            | RolloutPhase::Monitoring
    )
}

/// Read one release log's tail off the host.
///
/// The path is the one [`crate::release_agent`]'s `release_log` opens,
/// spelled by `release_agent::host_log_path` itself rather than retyped, so
/// the reader cannot look somewhere the writer does not write.
async fn stream_report(
    target: &crate::targets::ComputeTarget,
    logs_root: &str,
    product: &str,
    version: &str,
    extension: &'static str,
    lines: usize,
) -> Result<StreamReport, CmdError> {
    let path = host_log_path(logs_root, product, version, extension);
    let read = remote_read_tail(target, &path, lines).await?;
    Ok(match read {
        None => StreamReport {
            stream: extension,
            path,
            bytes: None,
            lines: Vec::new(),
            state: STREAM_MISSING,
        },
        Some((_, 0)) => StreamReport {
            stream: extension,
            path,
            bytes: Some(0),
            lines: Vec::new(),
            state: STREAM_EMPTY,
        },
        Some((tail, bytes)) => StreamReport {
            stream: extension,
            path,
            bytes: Some(bytes),
            lines: tail.lines().map(str::to_string).collect(),
            state: STREAM_READ,
        },
    })
}

async fn logs(args: &ReleaseLogsArgs) -> Result<(), CmdError> {
    if args.lines == 0 {
        return Err(CmdError::usage("--lines must be at least 1"));
    }
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, Some(args.target.as_str()))?;
    // The desired version is the version a candidate on this host is
    // running: the agent only ever stages what the registry desires. An
    // operator chasing a version that has since been rolled back names it
    // with `--version`.
    let version = match args.version.clone() {
        Some(version) => version,
        None => policy
            .desired
            .as_ref()
            .map(|desired| desired.version.clone())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} declares no desired release; name the version with --version",
                    args.product
                ))
            })?,
    };
    // `state_dir` and `logs_root` are absolute by registry contract
    // (`release_control::safe_absolute` refuses anything else), so there is
    // nothing to expand here.
    let logs_root = target_policy.logs_root.clone();
    let compute = compute_target(&target_name).await?;
    let mut streams = Vec::new();
    for extension in args.stream.extensions() {
        streams.push(
            stream_report(
                &compute,
                &logs_root,
                &args.product,
                &version,
                extension,
                args.lines,
            )
            .await?,
        );
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "version": version,
        "streams": streams.iter().map(StreamReport::to_value).collect::<Vec<Value>>(),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for stream in &streams {
        match stream.state {
            STREAM_MISSING => println!("--- {} ({}): no such file", stream.stream, stream.path),
            STREAM_EMPTY => println!(
                "--- {} ({}): present and empty — the agent opened it and the product \
                 wrote nothing",
                stream.stream, stream.path
            ),
            _ => {
                println!(
                    "--- {} ({}): last {} lines of {} bytes",
                    stream.stream,
                    stream.path,
                    stream.lines.len(),
                    stream.bytes.unwrap_or_default()
                );
                for line in &stream.lines {
                    println!("{line}");
                }
            }
        }
    }
    Ok(())
}

/// The candidate section: the port, whether the recorded pid is still there,
/// and what the readiness path answered.
///
/// Every field stays present with a `null` when there is nothing to probe,
/// because the desktop client reads a fixed shape and an absent key would
/// read as a missing candidate exactly where a dead one is the finding.
async fn candidate_section(
    target: &crate::targets::ComputeTarget,
    state: Option<&HostReleaseState>,
    readiness_path: Option<&str>,
) -> Result<Value, CmdError> {
    let Some(candidate) = state.and_then(|state| state.candidate.as_ref()) else {
        return Ok(json!({
            "port": Value::Null,
            "health_status": HEALTH_NO_CANDIDATE,
            "pid_alive": Value::Null,
        }));
    };
    let Some(readiness_path) = readiness_path else {
        return Ok(json!({
            "port": candidate.port,
            "health_status": HEALTH_UNPROBED,
            "pid_alive": Value::Null,
        }));
    };
    let script = candidate_probe_script(candidate.pid, candidate.port, readiness_path);
    let output = host_channel::run_script(target, &script, &production_runner())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut pid_alive = Value::Null;
    let mut health = HEALTH_UNREACHABLE.to_string();
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        if fields.first() != Some(&CANDIDATE_MARKER) || fields.len() != 3 {
            continue;
        }
        pid_alive = Value::from(fields[1] == "true");
        health = match fields[2].parse::<u16>() {
            Ok(status) if (200..300).contains(&status) => HEALTH_OK.to_string(),
            // curl writes `000` when it never got a response, which is the
            // same finding as an empty status: nothing answered.
            Ok(0) => HEALTH_UNREACHABLE.to_string(),
            Ok(status) => format!("http_{status}"),
            Err(_) => HEALTH_UNREACHABLE.to_string(),
        };
    }
    Ok(json!({
        "port": candidate.port,
        "health_status": health,
        "pid_alive": pid_alive,
    }))
}

/// The host's quarantine map, each entry told whether it is the digest the
/// registry currently desires.
///
/// `is_desired_digest` is the whole point of showing the map: a quarantined
/// digest that nobody desires is history, and the one that matches desired
/// state is a rollout that will be skipped on every pass until someone
/// clears it.
fn quarantine_entries(state: Option<&HostReleaseState>, desired_digest: Option<&str>) -> Vec<Value> {
    state.map_or_else(Vec::new, |state| {
        state
            .quarantined
            .iter()
            .map(|(digest, record)| {
                json!({
                    "digest": digest,
                    "reason": record.reason,
                    "quarantined_at": record.quarantined_at.to_rfc3339(),
                    "is_desired_digest": desired_digest == Some(digest.as_str()),
                })
            })
            .collect()
    })
}

async fn doctor(args: &ReleaseDoctorArgs) -> Result<(), CmdError> {
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, args.target.as_deref())?;
    let desired = policy.desired.as_ref();
    let desired_version = desired.map(|desired| desired.version.as_str());
    let desired_digest = desired
        .and_then(|desired| desired.artifacts.get(&target_policy.platform))
        .map(|artifact| artifact.artifact_sha256.as_str());
    let compute = compute_target(&target_name).await?;
    // The state file is read with the shared reader and parsed with the
    // agent's own parser, which checks the document's product and target
    // identity: a mistyped `--target` must fail, never diagnose one host
    // against another host's rollout.
    let state_path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let state = match remote_read(&compute, &state_path).await?.as_deref() {
        Some(payload) => Some(
            release_agent::parse_state_document(
                payload.as_bytes(),
                &args.product,
                &target_name,
                &state_path,
            )
            .map_err(CmdError::click)?,
        ),
        None => None,
    };
    // The published status row is what `stado release status` reads. It is
    // the fallback rather than the source here: it is written FROM the state
    // file this command already reads, and the publish itself can fail
    // (every one of them answered 401 for a week when the status URI named
    // an undeclared namespace), which is precisely when a diagnosis must
    // not go blind.
    let published: Value = match super::storage::fetch_object(&release_status_uri(
        &args.product,
        &target_name,
    ))
    .await
    {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let observed_version = state
        .as_ref()
        .and_then(|state| state.active.as_ref())
        .map(|record| record.version.clone())
        .or_else(|| {
            published["active_version"]
                .as_str()
                .map(str::to_string)
        });
    let phase = state.as_ref().map_or_else(
        || {
            published["phase"]
                .as_str()
                .map_or_else(|| PHASE_UNREPORTED.to_string(), str::to_string)
        },
        |state| phase_word(state.phase),
    );
    let detail = state.as_ref().map_or_else(
        || {
            published["detail"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        },
        |state| state.detail.clone(),
    );
    let candidate =
        candidate_section(&compute, state.as_ref(), target_policy.readiness_path.as_deref()).await?;
    let quarantined = quarantine_entries(state.as_ref(), desired_digest);
    // A failed gate read is a failed diagnosis, not a diagnosis with one
    // field missing. The Mac mini stopped claiming for hours on a gate
    // nothing reported; a verdict computed as if the gate were fine would
    // reproduce that incident with more confidence.
    let gates = host_gates::read_host_gates(&target_name, &production_runner())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut blockers: Vec<String> = gates.blockers.clone();
    if quarantined
        .iter()
        .any(|entry| entry["is_desired_digest"] == Value::Bool(true))
    {
        blockers.push(BLOCKER_DESIRED_DIGEST_QUARANTINED.to_string());
    }
    if candidate["health_status"].as_str().is_some_and(|status| {
        status != HEALTH_OK && status != HEALTH_NO_CANDIDATE && status != HEALTH_UNPROBED
    }) {
        blockers.push(BLOCKER_CANDIDATE_NOT_READY.to_string());
    }
    blockers.sort();
    blockers.dedup();

    let quarantine_blocks = blockers
        .iter()
        .any(|blocker| blocker == BLOCKER_DESIRED_DIGEST_QUARANTINED);
    let in_flight = state
        .as_ref()
        .is_some_and(|state| state.candidate.is_some() || phase_is_rolling(state.phase));
    let converged = observed_version.is_some() && observed_version.as_deref() == desired_version;
    let verdict = if quarantine_blocks || gates.disk_pressure_unresolved {
        VERDICT_BLOCKED
    } else if in_flight || !converged {
        VERDICT_ROLLING
    } else {
        VERDICT_SETTLED
    };

    let report = json!({
        "product": args.product,
        "target": target_name,
        "desired_version": desired_version,
        "observed_version": observed_version,
        "phase": phase,
        "detail": detail,
        "candidate": candidate,
        "quarantined": quarantined,
        "gates": host_gates::gates_section(&gates),
        "verdict": verdict,
        "blockers": blockers,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let cell = |value: &Value| match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    println!("product           {}", args.product);
    println!("target            {target_name}");
    println!("desired           {}", cell(&report["desired_version"]));
    println!("observed          {}", cell(&report["observed_version"]));
    println!("phase             {phase}");
    if !detail.is_empty() {
        println!("detail            {detail}");
    }
    println!(
        "candidate         port={} health={} pid_alive={}",
        cell(&report["candidate"]["port"]),
        cell(&report["candidate"]["health_status"]),
        cell(&report["candidate"]["pid_alive"])
    );
    println!(
        "gates             disk_pressure_unresolved={} free_gb={} low_watermark_gb={}",
        cell(&report["gates"]["disk_pressure_unresolved"]),
        cell(&report["gates"]["free_gb"]),
        cell(&report["gates"]["low_watermark_gb"])
    );
    println!("verdict           {verdict}");
    println!(
        "blockers          {}",
        if blockers.is_empty() {
            "none".to_string()
        } else {
            blockers.join(", ")
        }
    );
    if !quarantined.is_empty() {
        super::table::print(
            &["DIGEST", "DESIRED", "QUARANTINED AT", "REASON"],
            &quarantined
                .iter()
                .map(|entry| {
                    vec![
                        cell(&entry["digest"]),
                        cell(&entry["is_desired_digest"]),
                        cell(&entry["quarantined_at"]),
                        cell(&entry["reason"]),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    // The command that finishes the diagnosis, spelled out. In the incident
    // the state file's one sentence was the end of the trail; the log the
    // operator needed had a name nobody had written down.
    if verdict != VERDICT_SETTLED {
        if let Some(version) = desired_version {
            println!(
                "\nnext: stado release logs {} --target {target_name} --version {version} \
                 --stream err",
                args.product
            );
        }
    }
    Ok(())
}

pub async fn dispatch_logs(args: &ReleaseLogsArgs) -> Result<(), CmdError> {
    logs(args).await
}

pub async fn dispatch_doctor(args: &ReleaseDoctorArgs) -> Result<(), CmdError> {
    doctor(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_script_quotes_the_declared_readiness_path() {
        let script = candidate_probe_script(46748, 18080, "/health");
        assert!(script.starts_with("pid=46748\nport=18080\nreadiness_path='/health'\n"));
        assert!(script.contains("kill -0 \"$pid\""));
    }

    #[test]
    fn stderr_is_read_before_stdout() {
        assert_eq!(StreamArg::Both.extensions(), &["err", "out"]);
    }

    #[test]
    fn phase_word_matches_the_published_vocabulary() {
        assert_eq!(phase_word(RolloutPhase::CandidateRunning), "candidate_running");
        assert!(phase_is_rolling(RolloutPhase::CandidateRunning));
        assert!(!phase_is_rolling(RolloutPhase::Quarantined));
        assert!(!phase_is_rolling(RolloutPhase::Committed));
    }
}

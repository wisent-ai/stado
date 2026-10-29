//! What macOS decided about Local Network for programs on a host, read from
//! that host's unified log.
//!
//! Local Network is no TCC service: `nehelper` decides it, per code
//! identifier, and logs the decision it applies when a program reaches the
//! local network (`Local network allowed by preference for NAME
//! (IDENTIFIER)`). The host reads each declared program's identifier with
//! `codesign` and sends back the newest decision `nehelper` logged for it in
//! the window; a program that reached nothing on the local network in the
//! window has no decision to report.

use std::collections::BTreeMap;

use crate::deploy::{host_channel, shlex_quote, CommandOutput, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The report line naming one program's code identifier.
const PROGRAM_MARK: &str = "STADO_LN_PROGRAM";
/// The report line carrying one logged decision.
const DECISION_MARK: &str = "STADO_LN_DECISION";
/// The line saying the read ran to its end.
const END_MARK: &str = "STADO_LN_END";
/// The line saying `log show` itself failed, with its exit status.
const FAILED_MARK: &str = "STADO_LN_FAILED";

/// What `nehelper` last logged for one program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalNetwork {
    /// Allowed by the decision recorded for the program.
    Granted,
    /// Refused by the decision recorded for the program.
    Denied,
    /// macOS put the question to the person at the keyboard and logged no
    /// decision after it.
    Pending,
    /// The program reached nothing on the local network in the window.
    NotMeasured,
}

/// One program's decision and the code identifier it was judged under.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Decision {
    pub program: String,
    /// `codesign`'s identifier for the program on the host; none when the
    /// program is not there or carries no signature.
    pub identifier: Option<String>,
    pub state: LocalNetwork,
    /// When the decision was logged, in the host's local time.
    pub at: Option<String>,
}

/// Which stretch of a host's log the read covers.
pub enum Window<'a> {
    /// The last span, as `log show --last` takes it (`30m`, `2h`, `1d`).
    Last(&'a str),
    /// From a moment in the host's local time, `YYYY-MM-DD HH:MM:SS`.
    Since(&'a str),
}

/// The program the host runs: each program's identifier, then `nehelper`'s
/// Local Network lines of `window`, the newest per identifier.
fn script(programs: &[String], window: &Window) -> String {
    let quoted: Vec<String> = programs
        .iter()
        .map(|program| shlex_quote(program))
        .collect();
    let window = match window {
        Window::Last(span) => format!("--last {span}"),
        Window::Since(start) => format!("--start {}", shlex_quote(start)),
    };
    format!(
        r#"for program in {programs}; do
  # stderr: codesign's own display of the program; the identifier line is read
  # out of it, and a program that is not there names none.
  identifier=$(/usr/bin/codesign -dv "$program" 2>&1 | /usr/bin/sed -n 's/^Identifier=//p')
  printf '{PROGRAM_MARK}\t%s\t%s\n' "$program" "$identifier"
done
{{ /usr/bin/log show {window} --style compact --predicate 'process == "nehelper" AND eventMessage CONTAINS[c] "local network"'; printf 'STADO_LOG_STATUS %s\n' "$?"; }} |
  /usr/bin/awk '
    /^STADO_LOG_STATUS / {{ status = $2; next }}
    {{
      if (!match($0, /\([^()]*\)[,.]/)) next
      identifier = substr($0, RSTART + 1, RLENGTH - 3)
      verdict = "pending"
      if (index($0, "allowed by preference")) verdict = "granted"
      else if (index($0, "denied") || index($0, "not allowed")) verdict = "denied"
      else if (!index($0, "prompt")) next
      newest[identifier] = verdict "\t" $1 " " $2
    }}
    END {{
      if (status != "0") {{ printf "{FAILED_MARK}\t%s\n", status; exit 0 }}
      for (identifier in newest) printf "{DECISION_MARK}\t%s\t%s\n", identifier, newest[identifier]
      print "{END_MARK}"
    }}'
"#,
        programs = quoted.join(" "),
    )
}

/// The decisions the script's report lines carry, one per program asked
/// about, in the order asked.
fn parse(stdout: &str, stderr: &str) -> Result<Vec<Decision>, String> {
    let mut identifiers: Vec<(String, Option<String>)> = Vec::new();
    let mut decisions: BTreeMap<String, (LocalNetwork, String)> = BTreeMap::new();
    let mut ended = false;
    for line in stdout.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            [PROGRAM_MARK, program, identifier] => identifiers.push((
                program.to_string(),
                (!identifier.is_empty()).then(|| identifier.to_string()),
            )),
            [DECISION_MARK, identifier, verdict, at] => {
                let state = match *verdict {
                    "granted" => LocalNetwork::Granted,
                    "denied" => LocalNetwork::Denied,
                    _ => LocalNetwork::Pending,
                };
                decisions.insert(identifier.to_string(), (state, at.to_string()));
            }
            [FAILED_MARK, status] => {
                return Err(format!(
                    "`log show` of nehelper exited {status} on the host: {}",
                    stderr.trim()
                ))
            }
            [END_MARK] => ended = true,
            _ => {}
        }
    }
    if !ended {
        return Err(format!(
            "the host's Local Network read ended without its end line: {}",
            stderr.trim()
        ));
    }
    Ok(identifiers
        .into_iter()
        .map(|(program, identifier)| {
            let found = identifier.as_ref().and_then(|id| decisions.get(id));
            Decision {
                state: found.map_or(LocalNetwork::NotMeasured, |(state, _)| *state),
                at: found.map(|(_, at)| at.clone()),
                program,
                identifier,
            }
        })
        .collect())
}

/// What `target`'s `nehelper` last decided about Local Network for each of
/// `programs` (paths on that host) in the last `since`, read on the host
/// through its declared connection.
pub async fn decisions_on_host(
    target: &ComputeTarget,
    programs: &[String],
    since: &str,
    runner: &Runner,
) -> Result<Vec<Decision>, DeployError> {
    if programs.is_empty() {
        return Ok(Vec::new());
    }
    let output =
        host_channel::run_script(target, &script(programs, &Window::Last(since)), runner).await?;
    judged(output, &target.name)
}

/// What this machine's `nehelper` last decided about Local Network for each
/// of `programs` since `start` (local time, `YYYY-MM-DD HH:MM:SS`), read
/// here by the same script a host runs.
async fn decisions_here(
    programs: &[String],
    start: &str,
    runner: &Runner,
) -> Result<Vec<Decision>, DeployError> {
    let output = runner(CommandSpec {
        argv: vec!["/bin/bash".to_string(), "-s".to_string()],
        stdin: Some(script(programs, &Window::Since(start))),
    })
    .await
    .map_err(DeployError::unreachable)?;
    judged(output, "this machine")
}

/// The binary this process runs and what macOS last decided about Local
/// Network for it since it was placed: the decision every connection this
/// process opens to a LAN peer is judged by.
pub struct RunningBinary {
    pub program: String,
    /// When the binary was placed, in this machine's local time.
    pub placed: String,
    pub decision: Decision,
}

/// [`RunningBinary`] for this process. The log is read from the moment the
/// binary was placed, so an older binary's decision under the same code
/// identifier does not stand in for this one's; a binary or placement time
/// that cannot be read, or a log read that fails, is refused with its cause.
pub async fn running_binary(runner: &Runner) -> Result<RunningBinary, String> {
    let binary = std::env::current_exe()
        .map_err(|error| format!("which binary this is cannot be read: {error}"))?;
    let program = binary.to_string_lossy().into_owned();
    let placed = std::fs::metadata(&binary)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("when {program} was placed cannot be read: {error}"))?;
    let placed = chrono::DateTime::<chrono::Local>::from(placed)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let decision = decisions_here(std::slice::from_ref(&program), &placed, runner)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| format!("the Local Network read named nothing for {program}"))?;
    Ok(RunningBinary {
        program,
        placed,
        decision,
    })
}

/// The decisions a finished read reports, or why `place`'s read failed.
fn judged(output: CommandOutput, place: &str) -> Result<Vec<Decision>, DeployError> {
    if !output.succeeded() {
        return Err(DeployError(format!(
            "the Local Network read on {place} exited {}: {}",
            output.code,
            output.detail().trim()
        )));
    }
    parse(&output.stdout, &output.stderr).map_err(|cause| DeployError(format!("{place}: {cause}")))
}

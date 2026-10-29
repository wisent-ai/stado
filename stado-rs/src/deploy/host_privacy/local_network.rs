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

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
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

/// The program the host runs: each program's identifier, then `nehelper`'s
/// Local Network lines of the last `since`, the newest per identifier.
fn script(programs: &[String], since: &str) -> String {
    let quoted: Vec<String> = programs
        .iter()
        .map(|program| shlex_quote(program))
        .collect();
    format!(
        r#"for program in {programs}; do
  # stderr: codesign's own display of the program; the identifier line is read
  # out of it, and a program that is not there names none.
  identifier=$(/usr/bin/codesign -dv "$program" 2>&1 | /usr/bin/sed -n 's/^Identifier=//p')
  printf '{PROGRAM_MARK}\t%s\t%s\n' "$program" "$identifier"
done
{{ /usr/bin/log show --last {since} --style compact --predicate 'process == "nehelper" AND eventMessage CONTAINS[c] "local network"'; printf 'STADO_LOG_STATUS %s\n' "$?"; }} |
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
    let output = host_channel::run_script(target, &script(programs, since), runner).await?;
    if !output.succeeded() {
        return Err(DeployError(format!(
            "the Local Network read on {} exited {}: {}",
            target.name,
            output.code,
            output.detail().trim()
        )));
    }
    parse(&output.stdout, &output.stderr)
        .map_err(|cause| DeployError(format!("{}: {cause}", target.name)))
}

//! The loopback health probe. A probe ends on facts, never on a count or a
//! clock: a 2xx answer is healthy; a 4xx answer says the probe path or the
//! service is wrong and is refused at once; a destination unit that no
//! longer runs is not healthy, named with what its URL last answered; a unit
//! whose process listens on TCP ports none of which is the URL's is serving
//! elsewhere and is refused naming them; any other answer, while the unit
//! runs, is read again, and each answer that differs from the last is
//! printed so a waiting operator sees what the destination says.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::super::units::{listening, probe_unit};
use super::super::{marker_line, run_host_script};
use crate::cli::placement::candidates::managed_unit;
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementUnit;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// What the service at a probe URL answered once: its HTTP status as curl
/// writes it (`000` when nothing answered), and curl's exit status and error
/// when the request did not complete.
struct Answer {
    status: String,
    failure: Option<String>,
}

impl Answer {
    fn healthy(&self) -> bool {
        self.failure.is_none() && self.status.starts_with('2')
    }

    /// An HTTP 4xx: the service answered and refused the request itself, so
    /// reading the same URL again cannot turn it into a 2xx.
    /// https://www.rfc-editor.org/rfc/rfc9110#section-15.5
    fn refused_by_service(&self) -> bool {
        self.failure.is_none() && self.status.starts_with('4')
    }
}

impl PartialEq for Answer {
    fn eq(&self, other: &Self) -> bool {
        self.status == other.status && self.failure == other.failure
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            Some(failure) => write!(f, "no HTTP answer ({failure})"),
            None => write!(f, "HTTP {}", self.status),
        }
    }
}

/// A probe URL is loopback HTTP with an explicit port, as the profile
/// contract declares; its bytes travel to the host as base64, beside the
/// port the service must listen on.
fn probe_payload(url: &str) -> Result<(String, u16), CmdError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::declaration(format!("invalid placement probe URL: {error}")))?;
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    };
    match (parsed.scheme() == "http" && loopback, parsed.port()) {
        (true, Some(port)) => Ok((STANDARD.encode(url.as_bytes()), port)),
        _ => Err(CmdError::click(format!(
            "placement probe must use loopback HTTP with an explicit port: {url}"
        ))
        .stating(FailureCode::Config)),
    }
}

/// One request to the probe URL on `target`, answered whatever it returned.
async fn answer(
    target: &ComputeTarget,
    url: &str,
    payload: &str,
    runner: &Runner,
) -> Result<Answer, CmdError> {
    // curl reports a request that did not complete through --write-out
    // (exitcode, errormsg), so its own exit status is read there.
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
url=$(printf '%s' '{payload}' | /usr/bin/base64 "$decode")
/usr/bin/curl --silent --output /dev/null --write-out 'STADO_PLACEMENT_HEALTH\t%{{http_code}}\t%{{exitcode}}\t%{{errormsg}}\n' "$url" || :
"#
    );
    let output = run_host_script(target, &script, runner, "health probe").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_HEALTH\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: health probe returned no marker for {url}",
            target.name
        ))
        .stating(FailureCode::InfraDown)
    })?;
    let fields: Vec<&str> = line.split('\t').collect();
    let [_, status, exit, message] = fields.as_slice() else {
        return Err(CmdError::click(format!(
            "{}: malformed health probe marker for {url}: {line}",
            target.name
        ))
        .stating(FailureCode::InfraDown));
    };
    Ok(Answer {
        status: status.to_string(),
        failure: (!message.is_empty()).then(|| format!("curl exit {exit}: {message}")),
    })
}

/// The service at `url` on a running source answers 2xx now; any other
/// answer refuses the move before the source is fenced, naming it.
pub(in crate::cli::placement::transfer) async fn health_probe(
    target: &ComputeTarget,
    url: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let (payload, _) = probe_payload(url)?;
    let answer = answer(target, url, &payload, runner).await?;
    if answer.healthy() {
        return Ok(());
    }
    Err(
        CmdError::click(format!("{}: {url} answered {answer}", target.name))
            .stating(FailureCode::InfraDown),
    )
}

/// The started unit `spec` on `target` answers 2xx at `url`. Until it does,
/// the URL is read again while the unit's process runs, and each answer that
/// differs from the one before is printed. A 4xx answer refuses the move at
/// once, naming it and the URL; once the unit no longer runs, the move is
/// refused naming the unit and the URL's last answer. A unit whose process
/// listens on TCP ports none of which is the URL's is refused naming them.
/// While its process listens on no port it is still starting, and while it
/// listens on the URL's port its answer is the service's own statement; both
/// are read again, a restart of the process is printed, and the wait stands
/// in the register of open waits for as long as it lasts.
pub(in crate::cli::placement::transfer) async fn await_healthy(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    url: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let (payload, port) = probe_payload(url)?;
    let managed = managed_unit(spec)?;
    let what = format!("unit {} to answer 2xx at {url}", managed.unit);
    crate::wait::until(crate::wait::Kind::Host, what, &target.name, async {
        let mut previous: Option<Answer> = None;
        let mut process = None;
        loop {
            let answer = answer(target, url, &payload, runner).await?;
            if answer.healthy() {
                return Ok(());
            }
            if answer.refused_by_service() {
                return Err(CmdError::click(format!(
                    "{}: {url} answered {answer} while unit {} runs; the probe path or the \
                     service refuses the request, so the move does not wait for it",
                    target.name, managed.unit
                ))
                .stating(FailureCode::InfraDown));
            }
            if previous.as_ref() != Some(&answer) {
                println!(
                    "  {}: unit {} runs; {url} answered {answer}; reading it again",
                    target.name, managed.unit
                );
            }
            if !probe_unit(target, spec, runner).await?.running {
                return Err(CmdError::click(format!(
                    "{}: unit {} is no longer running; {url} last answered {answer}",
                    target.name, managed.unit
                ))
                .stating(FailureCode::InfraDown));
            }
            let sockets = listening(target, spec, runner).await?;
            for row in &sockets.unread {
                println!(
                    "  {}: socket table of unit {}: {row}",
                    target.name, managed.unit
                );
            }
            if !sockets.ports.is_empty() && !sockets.ports.contains(&port) {
                return Err(CmdError::click(format!(
                    "{}: unit {} listens on TCP {:?}, not on port {port} of {url}, which \
                     answered {answer}; the probe URL or the service's port is wrong",
                    target.name, managed.unit, sockets.ports
                ))
                .stating(FailureCode::InfraDown));
            }
            if let Some(was) = process.filter(|_| sockets.pid != process) {
                let now = match sockets.pid {
                    Some(pid) => format!("process {pid}"),
                    None => "no process".to_string(),
                };
                println!(
                    "  {}: unit {} restarted: process {was} is now {now}",
                    target.name, managed.unit
                );
            }
            process = sockets.pid;
            previous = Some(answer);
        }
    })
    .await
}

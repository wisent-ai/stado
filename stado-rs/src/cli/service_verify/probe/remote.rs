//! Another host's own vantage, read by asking that host's own stado.

use serde_json::Value;

use crate::observations::{OBSERVED, UNREACHABLE, UNVERIFIED};

use crate::cli::service_verify::probe::root_cause;
use crate::cli::service_verify::Finding;

/// The probe's one remote invocation: this same command in `--local` mode,
/// run by the host's own installed stado — nothing is installed for the read,
/// and nothing is left behind.
const PROBE_ARGV: &[&str] = &["service", "verify", "--local", "--json"];

/// Run the probe on one remote host, natively: locate the host's own stado,
/// then ask it for the local findings with the fixed [`PROBE_ARGV`].
///
/// Not `ssh`, and not `host exec`: the exec allowlist carries fixed read-only
/// argv and cannot express "and then interpret this URL", while an invocation
/// that took the URL as an argument would be a remote fetcher with the audit
/// trail removed. The probe takes no arguments at all -- it asks the same
/// registry this command is reading and probes the host's own share of it.
///
/// A host whose probe cannot run is reported `unverified`, never `observed`
/// and never `unreachable`. That is the whole point of the third state, so
/// every failure here is the detail string of the `unverified` rows.
async fn probe_remote(host: &str, runner: &crate::deploy::Runner) -> Result<String, String> {
    use crate::deploy::host_channel;
    let target = host_channel::canonical_target(host)
        .await
        .map_err(|error| root_cause(&error))?;
    let home = host_channel::remote_home(&target, runner)
        .await
        .map_err(|error| root_cause(&error))?;
    let stado = format!("{home}/.stado/bin/stado");
    if !host_channel::remote_test(
        &target,
        &format!("-x {}", crate::deploy::shlex_quote(&stado)),
        runner,
    )
    .await
    .map_err(|error| root_cause(&error))?
    {
        return Err(format!("missing executable Stado binary: {stado}"));
    }
    let mut words: Vec<&str> = vec![stado.as_str()];
    words.extend_from_slice(PROBE_ARGV);
    let output = host_channel::run_program(&target, &words, runner)
        .await
        .map_err(|error| root_cause(&error))?;
    // --local exits non-zero when a declaration is unreachable, which is the
    // answer the sweep wants recorded rather than treated as a broken probe —
    // the retired script's trailing `|| true`, taken here.
    Ok(output.stdout)
}

/// Run the probe on one remote host, through [`probe_remote`]: one fixed argv
/// answered by the host's own stado, with every refusal turned into the
/// `unverified` detail the sweep records.
pub(in crate::cli::service_verify) async fn remote_findings(
    host: &str,
    declared: &[(String, String)],
) -> Vec<Finding> {
    let runner = crate::deploy::production_runner();
    let unverified = |detail: String| -> Vec<Finding> {
        declared
            .iter()
            .map(|(service, endpoint)| Finding {
                service: service.clone(),
                host: host.to_string(),
                endpoint: endpoint.clone(),
                state: UNVERIFIED,
                detail: detail.clone(),
                probed: true,
            })
            .collect()
    };
    let output = match probe_remote(host, &runner).await {
        Ok(output) => output,
        Err(detail) => return unverified(detail),
    };
    let parsed: Value = match serde_json::from_str(output.trim()) {
        Ok(parsed) => parsed,
        Err(error) => return unverified(format!("probe returned no usable JSON: {error}")),
    };
    let Some(rows) = parsed.as_array() else {
        return unverified("probe returned no usable JSON: not an array".to_string());
    };
    rows.iter()
        // A standby row is a declaration, not evidence: identical on every
        // machine, and this sweep already read it out of the directory. Taking
        // the probe's copy as well would print the same address twice for a
        // row that has no vantage to be probed from. A remote stado older than
        // the flag sends no such rows and reports every one of its own as
        // probed, which is what it did.
        .filter(|row| row.get("probed").and_then(Value::as_bool).unwrap_or(true))
        .map(|row| Finding {
            service: field(row, "service"),
            host: host.to_string(),
            endpoint: field(row, "endpoint"),
            state: match row.get("state").and_then(Value::as_str) {
                Some(OBSERVED) => OBSERVED,
                Some(UNREACHABLE) => UNREACHABLE,
                _ => UNVERIFIED,
            },
            detail: field(row, "detail"),
            probed: true,
        })
        .collect()
}

fn field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

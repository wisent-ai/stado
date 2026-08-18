//! `stado service converge` — is the host running the version the registry
//! declares for it, and if not, put it there.
//!
//! Every other command in this group answers a question about a *unit*: is it
//! loaded, what does it run, what is in its environment, when did it last
//! restart. Not one of them could answer the question that actually cost this
//! fleet a day: **is the program on that host the build we shipped?** A
//! declaration named a label and a plist path, both of which stayed true across
//! every release that never reached the box, so a mac mini serving an old
//! version was byte-for-byte indistinguishable from one at the declared one —
//! `service list` said `active`, `service show` printed the same program path
//! it always had, and the beacons agreed. Nothing was wrong with any of those
//! answers. None of them was about the code.
//!
//! The primitive this compares is the one the fleet already delivers against:
//! `targets[].managed_versions`, the registry's per-binary statement of the
//! exact version a host must run. Not a git commit — the hosts do not carry
//! checkouts. `charless-mac-mini` runs Weles as an installed release artefact
//! with a `package.json`, a `.weles-release` stamp and a `provenance.json`
//! beside it and no `.git` anywhere, and a converge that compared commits
//! there could only ever report "unknown" about a product that is in fact
//! precisely versioned.
//!
//! Three verdicts, never two:
//!
//!   in-sync   the host runs exactly the declared version.
//!   drifted   the host runs a different version. This is the state that hid
//!             behind a passing `service list` for as long as it took somebody
//!             to notice the behaviour was old.
//!   unknown   the host said nothing usable: the reporting helper is not
//!             installed, the channel refused, or the artefact carries no
//!             version metadata at all. Kept apart from `drifted` for the same
//!             reason [`crate::cli::service_verify`] keeps `unverified` apart
//!             from `unreachable` — "I did not look" and "I looked and it is
//!             wrong" send an operator to two different places, and folding
//!             them together is how a fleet learns to ignore its own reports.
//!             It is never folded into `in-sync` either: an unmeasurable
//!             product is reported as unmeasured, in its own row, every time.
//!
//! The exit codes follow from that split, and the split is the whole reason
//! they differ:
//!
//! - **report mode** exits non-zero on `drifted` alone. A drifted host is a
//!   false declaration and a gate should fail on it; an uninstalled reporter is
//!   not evidence of anything and must not masquerade as drift, exactly as
//!   `service verify` refuses to let a missing probe masquerade as an outage.
//!   Every `unknown` row is still named on stderr, so nothing about it is
//!   silent.
//! - **`--apply`** exits non-zero unless every binary in scope came back
//!   `in-sync`. An operator who asked for convergence is owed proof of it, and
//!   "the reporter is not installed" is not proof — after an apply, an
//!   unconfirmed binary is a failed apply.
//!
//! Two things this command deliberately does not do. It never writes the
//! registry: the declared version is the operator's statement of intent,
//! published through `stado registry push` (`stado host declare-version`), and
//! a converge that edited the document to match the host would turn a drift
//! report into a rubber stamp. And it has no delivery mechanism of its own:
//! closing the gap is [`crate::deploy::host_release::release_host`], the exact
//! path `stado host release --binary NAME --version X.Y.Z TARGET` runs, called
//! in-process. One fetch, one digest check, one staging tree, one `rename(2)`,
//! one restart — for the command that reports drift and for the command that
//! delivers, because two ways to put a build on a host is one way too many.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::deploy::{
    host_channel, host_release, production_runner, shlex_quote, DeployError, Runner,
};
use crate::targets::ComputeTarget;

use super::{CmdError, CLICK_ERROR_CODE};

/// The host runs exactly the declared version.
pub const IN_SYNC: &str = "in-sync";
/// The host runs a version that is not the declared one.
pub const DRIFTED: &str = "drifted";
/// Nothing usable came back, so drift is neither confirmed nor ruled out.
pub const UNKNOWN: &str = "unknown";

/// The helper that reads every managed binary on the host and prints the
/// version it is actually installed at.
///
/// One program answering the same question for every binary on the box: a
/// per-product reporter would be a per-product opinion about what "the
/// installed version" means, and the whole point of `managed_versions` is that
/// there is one. It takes no arguments — `host run-helper` carries none — and
/// finds its own hostname in the canonical registry the same way this command
/// finds the host's declarations.
const VERSION_HELPER: &str = "report-installed-versions";

/// Where [`VERSION_HELPER`] comes from in this repository, so a host missing it
/// is told the exact command that fixes that rather than the fact alone.
const VERSION_HELPER_SOURCE: &str = "scripts/report-installed-versions.sh";

/// What the reporter prints for an artefact whose version it could not read,
/// and what this command prints back.
///
/// Spelled out because it is a wire value: the helper must be able to say "I
/// looked and could not tell" in a line that still names the binary, and a
/// blank, a dash or a truncated string would each be silently readable as
/// something else. Any value that is not an exact version lands as [`UNKNOWN`]
/// regardless; this constant is the one the helper is documented to send.
const UNKNOWN_VERSION: &str = "unknown";

/// What the reporter prints for a column that genuinely has no value — a
/// binary no declared unit runs, most of all. Distinct from
/// [`UNKNOWN_VERSION`]: "there is no unit" is a fact, "I could not read the
/// version" is the absence of one.
const NONE: &str = "none";

/// One declared binary, checked against what the host reported.
struct Row {
    binary: String,
    declared: String,
    /// The version the host reported, or `None` when nothing usable came back.
    /// `None` is the whole of [`UNKNOWN`] and is never collapsed into an empty
    /// string, which would compare unequal and read as drift.
    installed: Option<String>,
    /// Where on the host the reporter found the artefact it read.
    root: String,
    /// The declared unit whose program lives under `root`, or [`NONE`].
    unit: String,
    /// What launchd (or systemd) says about that unit.
    state: String,
    verdict: &'static str,
    detail: String,
}

impl Row {
    fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "declared_version": self.declared,
            "installed_version": self.installed,
            "root": self.root,
            "unit": self.unit,
            "state": self.state,
            "verdict": self.verdict,
            "detail": self.detail,
        })
    }

    /// The installed cell, in the words the table prints.
    fn installed_cell(&self) -> &str {
        self.installed.as_deref().unwrap_or(UNKNOWN_VERSION)
    }
}

/// What the reporter said about one binary.
#[derive(Default)]
struct Installed {
    /// `None` when the helper printed [`UNKNOWN_VERSION`], printed nothing
    /// usable, or printed something that is not an exact version.
    version: Option<String>,
    root: String,
    unit: String,
    state: String,
}

/// One `host release` invocation, run for one drifted binary.
struct Released {
    binary: String,
    version: String,
    status: &'static str,
    detail: String,
}

impl Released {
    fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "version": self.version,
            "status": self.status,
            "detail": self.detail,
        })
    }
}

/// What `--apply` found drifted and could do nothing about, kept apart from the
/// deliveries on purpose: a binary `host release` does not carry produced no
/// delivery at all, and counting it as a failed one would report an attempt
/// that never happened.
struct Undeliverable {
    binary: String,
    detail: String,
}

impl Undeliverable {
    fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "detail": self.detail,
        })
    }
}

const COMPLETED: &str = "completed";
const FAILED: &str = "failed";

/// Everything one `--apply` pass did: the releases it ran, and the drifted
/// binaries it could not run one for.
#[derive(Default)]
struct AppliedPass {
    releases: Vec<Released>,
    undeliverable: Vec<Undeliverable>,
}

fn click(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

/// `stado service converge TARGET [BINARY] [--apply]`.
pub async fn converge(
    target: &str,
    binary: Option<&str>,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(click)?;
    let declared = declaring(&resolved, binary)?;
    let runner = production_runner();

    let reported = read_installed(&resolved, &runner).await;
    let rows = verdict_rows(&declared, &reported);
    if !apply {
        emit(&resolved.name, None, &rows, json_output)?;
        return report_gate(&rows);
    }

    let pass = apply_releases(&resolved.name, &rows, &runner).await;
    // Re-read rather than trust delivery's own word for it. A `host release`
    // that reports `released` has testified about its own work, which is the
    // one witness that cannot establish the fact being claimed; the version the
    // host reports afterwards comes back through the same reporter that
    // produced the drift finding, so a successful delivery and a confirmed
    // convergence are not the same claim.
    let reported = read_installed(&resolved, &runner).await;
    let rows = verdict_rows(&declared, &reported);
    emit(&resolved.name, Some(&pass), &rows, json_output)?;
    apply_gate(&rows, &pass)
}

// ---------------------------------------------------------------------------
// What is declared
// ---------------------------------------------------------------------------

/// The binaries TARGET declares a version for, narrowed by BINARY.
///
/// Read straight off `targets[].managed_versions` through
/// [`ComputeTarget::declared_version`], the same accessor `host inventory` and
/// `host release` judge against: two readings of the declaration that can
/// disagree turn "the host is behind" and "the delivery is refused" into
/// independent answers to one question.
///
/// A declared version that is not an exact semantic version is refused here,
/// before the host is contacted at all, and so is a key someone emptied instead
/// of removing. `host release` refuses to deliver either one, so a comparison
/// against them could only ever produce drift no command in this pack can
/// close.
fn declaring(
    target: &ComputeTarget,
    binary: Option<&str>,
) -> Result<Vec<(String, String)>, CmdError> {
    let declared: Vec<(String, String)> = target
        .managed_versions
        .iter()
        .filter(|(name, _)| binary.is_none_or(|query| *name == query))
        .map(|(name, version)| (name.clone(), version.clone()))
        .collect();
    if declared.is_empty() {
        return Err(CmdError::click(match binary {
            Some(query) => format!(
                "{} declares no {query} version; `stado host declare-version {} \
                 --binary {query} --version X.Y.Z` states one. Delivery carries out a \
                 declaration, it does not stand in for one",
                target.name, target.name
            ),
            None => format!(
                "{} declares no {} at all, so nothing on it has a version to be in \
                 sync with; declare one with `stado host declare-version`",
                target.name,
                host_release::MANAGED_VERSIONS_KEY
            ),
        }));
    }
    for (name, version) in &declared {
        if !host_release::is_exact_semver(version) {
            return Err(CmdError::click(format!(
                "declared {name} version {version:?} on {} is not an exact \
                 semantic version such as 0.5.1; fix the declaration before comparing \
                 anything against it",
                target.name
            )));
        }
    }
    Ok(declared)
}

// ---------------------------------------------------------------------------
// What the host reports
// ---------------------------------------------------------------------------

/// Every version the host reported, keyed by binary name, or the reason nothing
/// was read.
///
/// The failure is one value for the whole host on purpose: when the reporter
/// cannot run, no binary on that box has a reported version, and the same
/// sentence belongs on every row rather than one row carrying the detail and
/// the rest carrying a blank.
///
/// The script is built by [`host_channel::installed_helper_script`] — the same
/// one `host run-helper` sends — so where a helper may live, and what makes one
/// acceptable to execute, is decided in exactly one place for both callers. No
/// arguments are appended at all: `run-helper` accepts UUIDs and nothing else,
/// and a helper reached from here has no correlation id to hand it, so the
/// argument string is empty and the helper reads the canonical registry to
/// learn which host it is reporting on. Reading a version is a status read and
/// nothing else, so it runs under the channel's ordinary read bound.
async fn read_installed(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BTreeMap<String, Installed>, String> {
    let script = host_channel::installed_helper_script(&shlex_quote(VERSION_HELPER), "");
    let output = host_channel::run_script_with_timeout(
        target,
        &script,
        host_channel::remote_timeout(),
        runner,
    )
    .await
    .and_then(|output| {
        if output.ok() {
            Ok(output)
        } else {
            Err(DeployError(host_channel::last_error_line(
                &output,
                "the installed helper did not complete",
            )))
        }
    });
    match output {
        Ok(output) => Ok(parse_report(&output.stdout)),
        Err(error) => Err(helper_failure(&target.name, error)),
    }
}

/// The reporter's own words, or the install line when it is simply not there.
///
/// A host that was never given the helper is the common case on the way in, and
/// "missing executable regular Stado helper: /Users/x/.stado/bin/..." tells an
/// operator what happened without telling them what to do about it. The install
/// command is the remedy, spelled out in full so it can be pasted.
fn helper_failure(target: &str, error: DeployError) -> String {
    let detail = error.to_string();
    if !detail.contains(host_channel::HELPER_MISSING) {
        return detail;
    }
    format!(
        "{VERSION_HELPER} is not installed on {target}; install it with \
         `stado host install-helper {target} {VERSION_HELPER_SOURCE} {VERSION_HELPER}`"
    )
}

/// The reporter's stdout, as a binary-to-report map.
///
/// Line-oriented `key=value` rather than JSON because the same output has to be
/// readable by an operator who ran the helper by hand on the box, and because a
/// shell script that has to emit valid JSON emits invalid JSON the first time a
/// path contains a quote. Blank lines and `#` comments are skipped, unknown
/// keys are ignored so the helper can add fields without a matching release
/// here, and only an exact version is kept: `version=unknown` — or anything
/// else that is not a semantic version — is the helper saying it could not
/// tell, which is [`UNKNOWN`] and never a comparison.
fn parse_report(stdout: &str) -> BTreeMap<String, Installed> {
    let mut reported = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut binary = None;
        let mut entry = Installed::default();
        let mut raw_version = "";
        for token in line.split_whitespace() {
            if let Some(value) = token.strip_prefix("binary=") {
                binary = Some(value);
            } else if let Some(value) = token.strip_prefix("version=") {
                raw_version = value;
            } else if let Some(value) = token.strip_prefix("root=") {
                entry.root = value.to_string();
            } else if let Some(value) = token.strip_prefix("unit=") {
                entry.unit = value.to_string();
            } else if let Some(value) = token.strip_prefix("state=") {
                entry.state = value.to_string();
            }
        }
        let Some(binary) = binary else {
            continue;
        };
        if host_release::is_exact_semver(raw_version) {
            entry.version = Some(raw_version.to_string());
        }
        reported.insert(binary.to_string(), entry);
    }
    reported
}

/// One row per declared binary, each carrying the verdict its two versions
/// imply.
fn verdict_rows(
    declared: &[(String, String)],
    reported: &Result<BTreeMap<String, Installed>, String>,
) -> Vec<Row> {
    declared
        .iter()
        .map(|(binary, declared_version)| {
            let entry = match reported {
                Ok(reported) => reported.get(binary),
                Err(_) => None,
            };
            let installed = entry.and_then(|entry| entry.version.clone());
            let (verdict, detail) = match (&installed, reported) {
                (Some(version), _) if version == declared_version => (IN_SYNC, String::from("-")),
                (Some(_), _) => (
                    DRIFTED,
                    String::from(
                        "the host runs a version the registry does not declare; --apply \
                         delivers the declared one through `stado host release`",
                    ),
                ),
                (None, Err(failure)) => (UNKNOWN, failure.clone()),
                (None, Ok(_)) => (
                    UNKNOWN,
                    match entry {
                        // Nothing to read a version out of. A different fact
                        // from an artefact that carries none, and a different
                        // remedy: install the product, rather than make it
                        // stamp itself.
                        Some(entry) if entry.root.is_empty() || entry.root == NONE => format!(
                            "{VERSION_HELPER} found no installed artefact for this \
                             binary on this host"
                        ),
                        // The reporter found the artefact and could not read a
                        // version out of it. Said in full, because the remedy
                        // is to make the product stamp its own artefact, not
                        // to re-run this command.
                        Some(entry) => format!(
                            "{VERSION_HELPER} found {} and no version metadata in it \
                             (package.json, .weles-release, provenance.json), so this \
                             host cannot be shown to run the declared version",
                            entry.root
                        ),
                        None => format!(
                            "{VERSION_HELPER} reported nothing for this binary; it is \
                             not installed on this host, or the helper could not find it"
                        ),
                    },
                ),
            };
            let cell = |value: Option<&str>| match value {
                Some(value) if !value.is_empty() => value.to_string(),
                _ => String::from(NONE),
            };
            Row {
                binary: binary.clone(),
                declared: declared_version.clone(),
                installed,
                root: cell(entry.map(|entry| entry.root.as_str())),
                unit: cell(entry.map(|entry| entry.unit.as_str())),
                state: cell(entry.map(|entry| entry.state.as_str())),
                verdict,
                detail,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Converging
// ---------------------------------------------------------------------------

/// Deliver the declared version of every drifted binary.
///
/// This is `stado host release --binary NAME --version X.Y.Z TARGET`, called
/// in-process rather than reimplemented: the digest check against the canonical
/// release manifest, the versioned staging tree, the `rename(2)` activation and
/// the unit restart all happen exactly once in this pack, and a second path to
/// "put a build on a host" is how two of them come to disagree about what a
/// verified artifact is.
///
/// A binary the registry declares but no product declaration carries is
/// recorded as undeliverable and never attempted: that refusal is made
/// against the shipped product declaration
/// ([`crate::deploy::products`]), so asking the host about it would cost an
/// ssh connection to learn something already known here.
///
/// `unknown` rows are deliberately not delivered. Nothing is known to be wrong
/// with them, delivery ends in a unit restart, and restarting a working service
/// on the strength of a reporter that failed to answer is how a healthy host
/// goes down because a report was missing.
async fn apply_releases(target: &str, rows: &[Row], runner: &Runner) -> AppliedPass {
    let mut pass = AppliedPass::default();
    for row in rows.iter().filter(|row| row.verdict == DRIFTED) {
        eprintln!(
            "{}: declared {} but runs {}",
            row.binary,
            row.declared,
            row.installed_cell()
        );
        if let Err(error) = crate::deploy::products::product(&row.binary) {
            pass.undeliverable.push(Undeliverable {
                binary: row.binary.clone(),
                detail: error.to_string(),
            });
            continue;
        }
        eprintln!("{target}: releasing {} {}", row.binary, row.declared);
        match host_release::release_host(target, &row.binary, &row.declared, false, runner).await {
            Ok(report) => {
                let status = report
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let delivered = matches!(
                    status.as_str(),
                    host_release::RELEASED_STATUS | host_release::ALREADY_ACTIVE_STATUS
                );
                pass.releases.push(Released {
                    binary: row.binary.clone(),
                    version: row.declared.clone(),
                    status: if delivered { COMPLETED } else { FAILED },
                    detail: if status.is_empty() {
                        String::from("the delivery reported no status")
                    } else {
                        status
                    },
                });
            }
            Err(error) => pass.releases.push(Released {
                binary: row.binary.clone(),
                version: row.declared.clone(),
                status: FAILED,
                detail: error.to_string(),
            }),
        }
    }
    pass
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// The report on stdout, and whatever `--apply` could not do on stderr.
///
/// `applied` is `None` in report mode, which is also what puts `"applied":
/// false` in the JSON: one value carries "was this a converge or a look", so
/// the two modes cannot disagree about which one produced the document.
fn emit(
    target: &str,
    applied: Option<&AppliedPass>,
    rows: &[Row],
    json_output: bool,
) -> Result<(), CmdError> {
    let empty = AppliedPass::default();
    let pass = applied.unwrap_or(&empty);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "applied": applied.is_some(),
                "releases": pass.releases.iter().map(Released::to_json).collect::<Vec<Value>>(),
                "undeliverable": pass
                    .undeliverable
                    .iter()
                    .map(Undeliverable::to_json)
                    .collect::<Vec<Value>>(),
                "binaries": rows.iter().map(Row::to_json).collect::<Vec<Value>>(),
            }))?
        );
        return Ok(());
    }
    println!(
        "{:<20} {:<12} {:<12} {:<9} {:<40} {:<10} DETAIL",
        "BINARY", "DECLARED", "INSTALLED", "VERDICT", "ROOT", "STATE"
    );
    for row in rows {
        println!(
            "{:<20} {:<12} {:<12} {:<9} {:<40} {:<10} {}",
            row.binary,
            row.declared,
            row.installed_cell(),
            row.verdict,
            row.root,
            row.state,
            row.detail
        );
    }
    for entry in pass.releases.iter().filter(|entry| entry.status == FAILED) {
        eprintln!("{} {}: {}", entry.binary, entry.version, entry.detail);
    }
    for entry in &pass.undeliverable {
        eprintln!("{}: {}", entry.binary, entry.detail);
    }
    Ok(())
}

/// Report mode: drift fails, an unmeasured binary does not.
///
/// This is what makes the command usable as a gate. A drifted host is a false
/// declaration and belongs in a non-zero exit; a host whose reporter is not
/// installed, or a product whose artefact carries no version metadata, has
/// produced no evidence either way, and turning that into a failure teaches
/// operators to pass `|| true`, at which point the drift the command exists to
/// catch stops being noticed again. Every such row is named on stderr instead,
/// because the one thing an unmeasured product must never be is quiet.
fn report_gate(rows: &[Row]) -> Result<(), CmdError> {
    for row in rows.iter().filter(|row| row.verdict == UNKNOWN) {
        eprintln!(
            "{}: declared {} and no installed version could be read — unmeasured, \
             not in sync: {}",
            row.binary, row.declared, row.detail
        );
    }
    let drifted = rows.iter().filter(|row| row.verdict == DRIFTED).count();
    if drifted == 0 {
        return Ok(());
    }
    eprintln!(
        "{drifted} declared binary/binaries run a version the registry does not \
         declare; re-run with --apply to deliver the declared one"
    );
    Err(CmdError::silent(CLICK_ERROR_CODE))
}

/// Apply mode: anything short of `in-sync` is a failed apply.
///
/// The operator asked for the host to be brought to the declared version, so
/// the only acceptable end state is one this command has confirmed by reading
/// the host again. `unknown` counts as failure here and does not in report
/// mode, and that is the intended difference: before an apply it means nobody
/// looked, after one it means the convergence cannot be shown to have happened.
fn apply_gate(rows: &[Row], pass: &AppliedPass) -> Result<(), CmdError> {
    let unresolved: Vec<&Row> = rows.iter().filter(|row| row.verdict != IN_SYNC).collect();
    if unresolved.is_empty() {
        return Ok(());
    }
    for row in &unresolved {
        eprintln!(
            "{}: declared {} != installed {}",
            row.binary,
            row.declared,
            row.installed_cell()
        );
    }
    let failed = pass
        .releases
        .iter()
        .filter(|entry| entry.status == FAILED)
        .count();
    // "no delivery ran" is a different diagnosis from "one ran and failed", and
    // both are different from "one ran, said it worked, and the host still
    // reports the old version" — and different again from "the drift is real
    // and nothing in this pack delivers that binary". The summary line names
    // which of the four this was, because the next action an operator takes
    // differs for every one of them.
    let mut effort = match (pass.releases.len(), failed) {
        (0, _) => String::from("no delivery ran"),
        (total, 0) => format!("{total} delivery/deliveries, none of which failed"),
        (total, failed) => format!("{total} delivery/deliveries, {failed} of which failed"),
    };
    if pass.undeliverable.is_empty() {
        if pass.releases.is_empty() {
            effort.push_str(", because nothing was confirmed drifted");
        }
    } else {
        effort.push_str(&format!(
            "; {} drifted binary/binaries are not deliverable by `stado host release`",
            pass.undeliverable.len()
        ));
    }
    eprintln!(
        "{} binary/binaries are not at their declared version after {effort}",
        unresolved.len()
    );
    Err(CmdError::silent(CLICK_ERROR_CODE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, version)| ((*name).to_string(), (*version).to_string()))
            .collect()
    }

    /// The reporter's contract, exercised on the three shapes one host really
    /// produces at once: a binary at its declared version, a binary at another
    /// one, and an artefact whose metadata carries no version at all.
    #[test]
    fn a_versionless_artefact_is_unknown_and_never_in_sync() {
        let stdout = "# host charless-mac-mini\n\
             binary=stado version=0.6.0 root=/Users/charles/.stado/bin/stado \
             unit=com.wisent.compute.agent.charless-mac-mini state=running\n\
             binary=skarbiec version=0.1.2 root=/Users/charles/.stado/bin/skarbiec \
             unit=none state=none\n\
             binary=weles-worker version=unknown root=/Users/charles/weles unit=none state=none\n";
        let reported = Ok(parse_report(stdout));
        let rows = verdict_rows(
            &declared(&[
                ("skarbiec", "0.1.3"),
                ("stado", "0.6.0"),
                ("weles-worker", "0.5.0"),
            ]),
            &reported,
        );

        let verdicts: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.binary.as_str(), row.verdict))
            .collect();
        assert_eq!(
            verdicts,
            vec![
                ("skarbiec", DRIFTED),
                ("stado", IN_SYNC),
                ("weles-worker", UNKNOWN),
            ]
        );
        // The unmeasured row keeps the host's own words for where it looked,
        // so the remedy names the artefact rather than the command.
        let weles = &rows[2];
        assert_eq!(weles.installed_cell(), UNKNOWN_VERSION);
        assert_eq!(weles.root, "/Users/charles/weles");
        assert!(
            weles.detail.contains("/Users/charles/weles"),
            "detail must name the artefact: {}",
            weles.detail
        );

        // Report mode fails on the drifted binary alone; the unknown one is
        // reported, not counted.
        assert!(report_gate(&rows).is_err());
        assert!(report_gate(&rows[1..2]).is_ok());
        assert!(report_gate(&rows[2..]).is_ok());

        // After an apply, an unconfirmed binary is a failed apply.
        assert!(apply_gate(&rows[1..2], &AppliedPass::default()).is_ok());
        assert!(apply_gate(&rows[2..], &AppliedPass::default()).is_err());
    }

    /// A host that never answered leaves every row unknown, carrying the same
    /// sentence — including the install line for a helper that is not there.
    #[test]
    fn an_unanswered_host_is_unknown_everywhere_with_the_install_line() {
        let failure = helper_failure(
            "charless-mac-mini",
            DeployError(format!(
                "{}: /Users/charles/.stado/bin/{VERSION_HELPER}",
                host_channel::HELPER_MISSING
            )),
        );
        let rows = verdict_rows(
            &declared(&[("skarbiec", "0.1.3"), ("stado", "0.6.0")]),
            &Err(failure),
        );
        assert!(rows.iter().all(|row| row.verdict == UNKNOWN));
        assert!(
            rows[0].detail.contains(&format!(
                "stado host install-helper charless-mac-mini {VERSION_HELPER_SOURCE} \
                 {VERSION_HELPER}"
            )),
            "the remedy must be pasteable: {}",
            rows[0].detail
        );
        assert!(report_gate(&rows).is_ok());
        assert!(apply_gate(&rows, &AppliedPass::default()).is_err());
    }

    /// A version the helper could not have read — a truncated line, a `null`,
    /// a two-component version — is unknown, never a comparison that reads as
    /// drift and never one that reads as agreement.
    #[test]
    fn a_malformed_reported_version_is_unknown() {
        let reported = Ok(parse_report(
            "binary=stado version=0.6 root=/x unit=none state=none\n\
             binary=skarbiec root=/y unit=none state=none\n",
        ));
        let rows = verdict_rows(
            &declared(&[("skarbiec", "0.1.3"), ("stado", "0.6.0")]),
            &reported,
        );
        assert!(rows.iter().all(|row| row.verdict == UNKNOWN));
        assert_eq!(rows[1].root, "/x");
    }
}

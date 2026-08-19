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
//!   unknown   the host said nothing usable: the reporter could not run, the
//!             channel refused, or the artefact carries no
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

use crate::deploy::service;
use crate::deploy::{host_channel, host_release, production_runner, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::{CmdError, CLICK_ERROR_CODE};

/// The host runs exactly the declared version.
pub const IN_SYNC: &str = "in-sync";
/// The host runs a version that is not the declared one.
pub const DRIFTED: &str = "drifted";
/// Nothing usable came back, so drift is neither confirmed nor ruled out.
pub const UNKNOWN: &str = "unknown";

/// The reporter that reads every managed binary on the host and prints the
/// version it is actually installed at, embedded in this binary and run as one
/// fixed remote script.
///
/// One program answering the same question for every binary on the box: a
/// per-product reporter would be a per-product opinion about what "the
/// installed version" means, and the whole point of `managed_versions` is that
/// there is one. It takes no arguments and finds its own hostname in the
/// canonical registry the same way this command finds the host's declarations.
/// Kept as a checked-in file rather than a string literal so it is reviewed and
/// read as the shell program it is.
const VERSION_PROBE: &str = include_str!("../../scripts/report-installed-versions.sh");

/// The reporter's name, for sentences that need to name it.
const VERSION_HELPER: &str = "report-installed-versions";

/// What the reporter prints for an artefact whose version it could not read,
/// and what this command prints back.
///
/// Spelled out because it is a wire value: the reporter must be able to say "I
/// looked and could not tell" in a line that still names the binary, and a
/// blank, a dash or a truncated string would each be silently readable as
/// something else. Any value that is not an exact version lands as [`UNKNOWN`]
/// regardless; this constant is the one the reporter is documented to send.
const UNKNOWN_VERSION: &str = "unknown";

/// What the reporter prints for a column that genuinely has no value — a
/// binary no declared unit runs, most of all. Distinct from
/// [`UNKNOWN_VERSION`]: "there is no unit" is a fact, "I could not read the
/// version" is the absence of one.
const NONE: &str = "none";

/// The process column's word for a live process executing the artefact the
/// unit's declaration resolves to.
const PROCESS_MATCHES: &str = "matches";

/// The process column's word for a live process executing something else. The
/// verdict beside it can be `in-sync` at the same time, and that combination is
/// the whole reason the column exists: the version on disk is the declared one
/// and the running code is not it.
const PROCESS_DIFFERS: &str = "differs";

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
    /// The executable the live process under `unit` is running, or `None` when
    /// no process was found to ask about.
    running_binary: Option<String>,
    /// Whether that process is executing the artefact the unit's declaration
    /// resolves to; `None` when it could not be established.
    ///
    /// Every other answer in this command is about what is INSTALLED, and an
    /// installed version says nothing about a process that started before it.
    /// Two production incidents sat in that gap with every other column
    /// correct: Brama's process kept running an artefact tree `current` no
    /// longer pointed at, and the Weles worker kept serving a `dist` replaced
    /// 26 seconds after it started. See
    /// [`crate::deploy::service::RunningProgram::matches_process`].
    binary_matches_process: Option<bool>,
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
            "running_binary": self.running_binary,
            "binary_matches_process": self.binary_matches_process,
            "verdict": self.verdict,
            "detail": self.detail,
        })
    }

    /// The installed cell, in the words the table prints.
    fn installed_cell(&self) -> &str {
        self.installed.as_deref().unwrap_or(UNKNOWN_VERSION)
    }

    /// The process cell. [`UNKNOWN`] for a unit nothing could be observed
    /// about, never folded into either of the other two words, for the same
    /// reason the verdict column keeps its own `unknown`.
    fn process_cell(&self) -> &'static str {
        match self.binary_matches_process {
            Some(true) => PROCESS_MATCHES,
            Some(false) => PROCESS_DIFFERS,
            None => UNKNOWN,
        }
    }
}

/// What the reporter said about one binary.
#[derive(Default)]
struct Installed {
    /// `None` when the reporter printed [`UNKNOWN_VERSION`], printed nothing
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
    let mut rows = verdict_rows(&declared, &reported);
    attach_processes(&resolved, &mut rows, &runner).await;
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
    let mut rows = verdict_rows(&declared, &reported);
    // Asked again after the delivery for the same reason the versions are: a
    // release ends in a restart, and whether the restarted process is executing
    // the artefact that was just installed is exactly the claim `--apply` is
    // being asked to prove.
    attach_processes(&resolved, &mut rows, &runner).await;
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
/// The reporter is [`VERSION_PROBE`], embedded in this binary — the script
/// travels with stado, so there is nothing to install on the host and the
/// failure text is the remote's own words, never a remedy for a delivery
/// channel that no longer exists. No arguments are appended at all: the probe
/// reads the canonical registry to learn which host it is reporting on.
/// Reading a version is a status read and nothing else, so it runs under the
/// channel's ordinary read bound.
async fn read_installed(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BTreeMap<String, Installed>, String> {
    let output = host_channel::run_script_with_timeout(
        target,
        VERSION_PROBE,
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
                "the version reporter did not complete",
            )))
        }
    });
    match output {
        Ok(output) => Ok(parse_report(&output.stdout)),
        Err(error) => Err(error.to_string()),
    }
}

/// The reporter's stdout, as a binary-to-report map.
///
/// Line-oriented `key=value` rather than JSON because a shell script that has
/// to emit valid JSON emits invalid JSON the first time a path contains a
/// quote. Blank lines and `#` comments are skipped, unknown keys are ignored so
/// the reporter can add fields without a matching release here, and only an
/// exact version is kept: `version=unknown` — or anything else that is not a
/// semantic version — is the reporter saying it could not tell, which is
/// [`UNKNOWN`] and never a comparison.
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
                             not installed on this host, or the reporter could not find it"
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
                // Filled by [`attach_processes`], which asks the host a second
                // question. Left empty here so the version comparison — the
                // answer this command exists for — never depends on a process
                // lookup having succeeded.
                running_binary: None,
                binary_matches_process: None,
                verdict,
                detail,
            }
        })
        .collect()
}

/// Ask the host which artefact the live process under each named unit is
/// executing, and fill the two process fields of every row it answers for.
///
/// A second read on the same channel rather than two more fields on the version
/// reporter, because they are two different questions: the reporter answers what
/// is INSTALLED, this answers what is RUNNING, and the incidents that motivate
/// this column are precisely the cases where those two disagree while every
/// other column is correct.
///
/// One round trip per distinct unit, and only for units a row actually names: a
/// declared binary no unit runs has no process to ask about. A lookup that fails
/// leaves both fields `None` and nothing else changes — refusing to print the
/// version comparison because a secondary read failed would trade this
/// command's whole purpose against an addition to it.
async fn attach_processes(target: &ComputeTarget, rows: &mut [Row], runner: &Runner) {
    let declared = service::declared_services(target);
    let mut asked: BTreeMap<String, Option<service::RunningProgram>> = BTreeMap::new();
    for row in rows.iter_mut() {
        if row.unit.is_empty() || row.unit == NONE {
            continue;
        }
        if !asked.contains_key(&row.unit) {
            // A unit the reporter named and the registry does not declare is
            // not asked about at all: locating its unit file would mean
            // guessing a path for a unit nobody adopted, which is the one
            // thing `service adopt` exists to stop.
            let found = declared
                .iter()
                .find(|candidate| candidate.matches(&row.unit));
            let program = match found {
                Some(service) => service::inspect_process(target, service, runner).await.ok(),
                None => None,
            };
            asked.insert(row.unit.clone(), program);
        }
        if let Some(Some(program)) = asked.get(&row.unit) {
            row.running_binary = program.running_binary().map(str::to_string);
            row.binary_matches_process = program.matches_process();
        }
    }
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
        "{:<20} {:<12} {:<12} {:<9} {:<40} {:<10} {:<8} DETAIL",
        "BINARY", "DECLARED", "INSTALLED", "VERDICT", "ROOT", "STATE", "PROCESS"
    );
    for row in rows {
        println!(
            "{:<20} {:<12} {:<12} {:<9} {:<40} {:<10} {:<8} {}",
            row.binary,
            row.declared,
            row.installed_cell(),
            row.verdict,
            row.root,
            row.state,
            row.process_cell(),
            row.detail
        );
    }
    // The path is what an operator acts on and is far too long for a column, so
    // it is named here — and only for the rows where it contradicts the
    // declaration, which are the rows that would otherwise read as fine.
    for row in rows
        .iter()
        .filter(|row| row.process_cell() == PROCESS_DIFFERS)
    {
        eprintln!(
            "{}: the process under {} is running {} — not the artefact this \
             unit's declaration resolves to; restart it to pick up what is installed",
            row.binary,
            row.unit,
            row.running_binary.as_deref().unwrap_or(UNKNOWN)
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


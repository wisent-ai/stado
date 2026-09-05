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
//! checkouts. `control-host` runs Weles as an installed release artefact
//! with a `package.json`, a `.weles-release` stamp and a `provenance.json`
//! beside it and no `.git` anywhere, and a converge that compared commits
//! there could only ever report "unknown" about a product that is in fact
//! precisely versioned.
//!
//! Four verdicts, never two:
//!
//!   unattested  the host runs bytes whose provenance cannot be shown: the
//!               version they claim has no delivered copy staged on the host,
//!               or the installed file is not that copy. Judged BEFORE drift,
//!               because a version string is not provenance and reading a
//!               local build as `host-ahead` is how it offers to write its own
//!               version into the registry.
//!   in-sync     the host runs exactly the declared version.
//!   host-behind the host runs a version strictly OLDER than the declared
//!               one. This is the state that hid behind a passing
//!               `service list` for as long as it took somebody to notice
//!               the behaviour was old. `--apply` delivers the declared
//!               version through `stado host release`.
//!   host-ahead  the host runs a version strictly NEWER than the declared
//!               one: the declaration is the thing that is stale. Delivering
//!               the declared version here would DOWNGRADE a live host, so
//!               `--apply` refuses to touch it and names the
//!               `stado host declare-version` command that moves the
//!               declaration to the version the host is actually running.
//!   unknown     the host said nothing usable: the reporter could not run, the
//!               channel refused, or the artefact carries no
//!               version metadata at all. Kept apart from both drift verdicts
//!               for the same
//!               reason [`crate::cli::service_verify`] keeps `unverified` apart
//!             from `unreachable` — "I did not look" and "I looked and it is
//!             wrong" send an operator to two different places, and folding
//!             them together is how a fleet learns to ignore its own reports.
//!             It is never folded into `in-sync` either: an unmeasurable
//!             product is reported as unmeasured, in its own row, every time.
//!
//! The exit codes follow from that split, and the split is the whole reason
//! they differ:
//!
//! - **report mode** exits non-zero on `host-behind` or `host-ahead` alone.
//!   Either is a false declaration and a gate should fail on it; an
//!   uninstalled reporter is
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

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::deploy::service;
use crate::deploy::{host_channel, host_release, production_runner, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::{CmdError, CLICK_ERROR_CODE};

/// The host runs exactly the declared version.
pub const IN_SYNC: &str = "in-sync";
/// The host runs a version strictly OLDER than the declared one: the host is
/// behind the declaration and `--apply` delivers the declared one.
pub const HOST_BEHIND: &str = "host-behind";
/// The host runs a version strictly NEWER than the declared one: the
/// declaration is the thing that is stale, and delivering it would take the
/// host backwards, so `--apply` refuses to touch the host at all.
pub const HOST_AHEAD: &str = "host-ahead";
/// Nothing usable came back, so drift is neither confirmed nor ruled out.
pub const UNKNOWN: &str = "unknown";
/// The host runs bytes this fleet cannot attest: the version they claim has
/// no delivered copy staged on the host, or the installed file differs from
/// the staged one it should have been installed from.
///
/// A version number is not provenance. `--version` prints whatever
/// `Cargo.toml` said when the file was compiled, so a local build reports a
/// release number it never came from, and this command used to read exactly
/// that as [`HOST_AHEAD`] — "the declaration is stale, not the host" — and
/// offer to write the unverified version into the registry. On 2026-08-31
/// charless-mac-mini, the always-on Mac every other host reads its registry
/// from, was running a `stado` answering 0.13.19 written at 21:25Z while the
/// 0.13.19 coordinate measured present=0 / absent=9 on both platforms: bytes
/// nobody delivered, one `--apply` away from promoting themselves into the
/// fleet's own record of what that host runs.
///
/// The delivery path stages every release it installs at
/// `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, digest-
/// verified against the canonical manifest on the way in. So the attestation
/// is host-local and needs no network: the staged copy for the claimed
/// version either exists and matches the installed file byte for byte, or
/// this verdict says so.
pub const UNATTESTED: &str = "unattested";

/// The staged copy exists and the installed file matches it.
const ATTEST_MATCH: &str = "staged-match";
/// A staged copy for the claimed version exists and the installed file is not
/// it: the binary was replaced after delivery.
const ATTEST_DIFFERS: &str = "staged-differs";
/// No staged copy for the claimed version: these bytes never came through the
/// delivery path.
const ATTEST_ABSENT: &str = "no-staged-copy";
/// No staged copy for the claimed version AND no staged copy of this binary
/// at any version: the delivery path has never run here for it.
///
/// Held apart from [`ATTEST_ABSENT`] because the two carry opposite
/// histories and opposite remedies, and folding them together made the
/// verdict unreadable. On 2026-09-01 `lukasz-macbook` reported both at once:
/// `skarbiec` had no `~/.stado/releases/skarbiec` directory at all — the
/// bootstrap installer stages nothing, so a binary that has never been
/// delivered reads exactly like one that was tampered with — while `stado`
/// had nine staged versions, the newest `0.13.24` from the day before, and a
/// `0.13.28` at the install path that no delivery put there. One is a host
/// nobody has released to yet; the other is a binary swapped in beside a
/// working pipeline. Printing the same sentence for both is what made
/// "unattested" look like the normal state of every host.
const ATTEST_NEVER_DELIVERED: &str = "no-delivery-history";
/// The version could not be read, so provenance was never asked.
const ATTEST_UNKNOWN: &str = "unknown";

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
    /// One of [`ATTEST_MATCH`], [`ATTEST_DIFFERS`], [`ATTEST_ABSENT`] or
    /// [`ATTEST_UNKNOWN`]: whether the installed file is the one the delivery
    /// path staged for the version it claims.
    attestation: String,
    /// What the delivery receipt beside the staged copy says, underscored for
    /// the wire. Empty when the delivery predates receipts, which is not a
    /// finding: the byte comparison attests those bytes without it.
    receipt: String,
}

/// One `host release` invocation, run for one `host-behind` binary.
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

/// What `--apply` found behind its declaration and could do nothing about,
/// kept apart from the
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

/// What `--apply` refused to do: the host runs a version strictly NEWER than
/// the declaration, so delivering the declared one would be a downgrade of a
/// live host. Kept apart from both the deliveries and the undeliverable:
/// nothing was attempted, and the remedy moves the declaration, not the host.
struct Refused {
    binary: String,
    declared: String,
    installed: String,
    /// The exact command that moves the declaration to the observed version.
    remediation: String,
}

impl Refused {
    fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "declared_version": self.declared,
            "installed_version": self.installed,
            "remediation": self.remediation,
        })
    }
}

const COMPLETED: &str = "completed";
const FAILED: &str = "failed";

/// Everything one `--apply` pass did: the releases it ran, the `host-behind`
/// binaries it could not run one for, and the downgrades it refused.
#[derive(Default)]
struct AppliedPass {
    releases: Vec<Released>,
    undeliverable: Vec<Undeliverable>,
    refused: Vec<Refused>,
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

    let mut pass = apply_releases(&resolved.name, &rows, &runner).await;
    // Re-read rather than trust delivery's own word for it. A `host release`
    // that reports `released` has testified about its own work, which is the
    // one witness that cannot establish the fact being claimed; the version the
    // host reports afterwards comes back through the same reporter that
    // produced the drift finding, so a successful delivery and a confirmed
    // convergence are not the same claim.
    let reported = read_installed(&resolved, &runner).await;
    let mut rows = verdict_rows(&declared, &reported);
    let stado_root_in_sync = rows.iter().any(|row| {
        row.binary == "stado"
            && row.verdict == IN_SYNC
            && reported
                .as_ref()
                .ok()
                .and_then(|entries| entries.get("stado"))
                .is_some_and(|entry| entry.attestation == ATTEST_MATCH)
    });
    if stado_root_in_sync {
        converge_native_readers(&resolved, &declared, &runner, &mut pass).await;
    }
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
/// The reporter is [`probe_installed_versions`]: the checks the retired probe
/// script ran, as individual remote commands with every branch taken here, so
/// there is nothing to install on the host and the failure text is the
/// remote's own words, never a remedy for a delivery channel that no longer
/// exists. The declarations the probe compares against are the registry this
/// command already resolved — the same canonical registry the retired script
/// re-read on the host to learn which host it was reporting on. Reading a
/// version is a status read and nothing else, so every remote command runs
/// under the channel's ordinary read bound.
async fn read_installed(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BTreeMap<String, Installed>, String> {
    match probe_installed_versions(target, runner).await {
        Ok(stdout) => Ok(parse_report(&stdout)),
        Err(error) => Err(error.to_string()),
    }
}

/// The services one registry target declares, as `(label, path, kind)` —
/// label falling back to unit and then to name, fields the retired probe
/// read out of the registry document with python. Used only to attribute a
/// unit to an artefact, never to decide a version.
fn declared_service_records(target: &ComputeTarget) -> Vec<(String, String, String)> {
    let text = |record: &serde_json::Map<String, Value>, key: &str| match record.get(key) {
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    };
    target
        .extra
        .get(service::SERVICES_KEY)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(Value::as_object)
                .map(|record| {
                    let label = ["label", "unit", "name"]
                        .iter()
                        .map(|key| text(record, key))
                        .find(|value| !value.is_empty())
                        .unwrap_or_default();
                    (label, text(record, "path"), text(record, "kind"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The program one declared unit runs, read out of the unit file itself.
async fn unit_program(
    target: &ComputeTarget,
    runner: &Runner,
    path: &str,
    kind: &str,
) -> Result<Option<String>, DeployError> {
    if !host_channel::remote_test(
        target,
        &format!("-f {}", crate::deploy::shlex_quote(path)),
        runner,
    )
    .await?
    {
        return Ok(None);
    }
    if kind == "systemd" {
        let read = host_channel::run_command(
            target,
            &format!(
                "sed -n 's/^ExecStart=//p' {} | head -n 1",
                crate::deploy::shlex_quote(path)
            ),
            runner,
        )
        .await?;
        return Ok(read
            .stdout
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .map(str::to_string));
    }
    let extracted = host_channel::run_program(
        target,
        &[
            "/usr/bin/plutil",
            "-extract",
            "ProgramArguments.0",
            "raw",
            "-o",
            "-",
            path,
        ],
        runner,
    )
    .await?;
    let program = extracted.stdout.trim();
    Ok((extracted.ok() && !program.is_empty()).then(|| program.to_string()))
}

/// The declared unit whose program lives under this artefact, or nothing.
///
/// Matched on the program the unit file actually names rather than on the
/// binary's name: a label that merely mentions "stado" is a guess, and a
/// wrong unit in a report is worse than an admitted absence.
async fn unit_for_root(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    services: &[(String, String, String)],
    root: &str,
) -> Result<Option<(String, String, String)>, DeployError> {
    if root.is_empty() {
        return Ok(None);
    }
    for (label, path, kind) in services {
        if label.is_empty() {
            continue;
        }
        let path = path
            .strip_prefix("$HOME/")
            .map_or_else(|| path.clone(), |rest| format!("{home}/{rest}"));
        let Some(program) = unit_program(target, runner, &path, kind).await? else {
            continue;
        };
        if program == root || program.starts_with(&format!("{root}/")) {
            return Ok(Some((label.clone(), path, kind.clone())));
        }
    }
    Ok(None)
}

/// launchd state for one label, from `launchctl print` and, when the domain
/// refuses it, from `launchctl list`. Spaces are folded to dashes so a state
/// like `spawn scheduled` stays one token.
async fn launchd_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
    domain: &str,
) -> Result<String, DeployError> {
    let printed = host_channel::run_program(
        target,
        &["/bin/launchctl", "print", &format!("{domain}/{label}")],
        runner,
    )
    .await?;
    let value = if printed.ok() {
        printed
            .stdout
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "state").then(|| value.trim_start().to_string())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "loaded".to_string())
    } else {
        let listed = host_channel::run_program(target, &["/bin/launchctl", "list"], runner).await?;
        listed
            .stdout
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?;
                fields.next()?;
                let name = fields.next()?;
                (name == label).then(|| {
                    if pid == "-" {
                        "loaded-not-running".to_string()
                    } else {
                        format!("running-pid-{pid}")
                    }
                })
            })
            .unwrap_or_else(|| "not-loaded".to_string())
    };
    Ok(value.replace(' ', "-"))
}

/// systemd state for one unit, or `no-systemctl` on a host without systemd.
async fn systemd_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
) -> Result<String, DeployError> {
    let found = host_channel::run_command(target, "command -v systemctl", runner).await?;
    if found.stdout.trim().is_empty() {
        return Ok("no-systemctl".to_string());
    }
    let asked =
        host_channel::run_program(target, &["systemctl", "is-active", label], runner).await?;
    Ok(asked.stdout.trim().to_string())
}

/// The state of one unit, by its kind and where its unit file lives:
/// LaunchDaemons print in the system domain, everything else in the login
/// user's GUI domain.
async fn unit_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
    path: &str,
    kind: &str,
    uid: &mut Option<String>,
) -> Result<String, DeployError> {
    if kind == "systemd" {
        return systemd_state(target, runner, label).await;
    }
    if path.starts_with("/Library/LaunchDaemons/") {
        return launchd_state(target, runner, label, "system").await;
    }
    if uid.is_none() {
        let answered = host_channel::run_program(target, &["/usr/bin/id", "-u"], runner).await?;
        *uid = Some(answered.stdout.trim().to_string());
    }
    launchd_state(
        target,
        runner,
        label,
        &format!("gui/{}", uid.as_deref().unwrap_or_default()),
    )
    .await
}

/// Where an installed product lives, or nothing. Candidates and never a
/// search: a probe that walks the filesystem looking for something called
/// <name> finds a backup copy and reports its version as the running one.
async fn artefact_root(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    name: &str,
) -> Result<String, DeployError> {
    let stem = name.split('-').next().unwrap_or(name);
    for candidate in [
        format!("{home}/{name}"),
        format!("{home}/{stem}"),
        format!("{home}/.stado/releases/{name}/current"),
        format!("/opt/{name}"),
    ] {
        if host_channel::remote_test(
            target,
            &format!("-d {}", crate::deploy::shlex_quote(&candidate)),
            runner,
        )
        .await?
        {
            return Ok(candidate);
        }
    }
    Ok(String::new())
}

/// The version an installed release artefact carries about itself:
/// `package.json` first — the version source a released product declares for
/// itself in `.wisent-release.json` — then the `.weles-release` stamp the
/// release launcher writes beside the unpacked runtime, then the SLSA
/// `provenance.json` shipped inside the artefact.
async fn artefact_version(
    target: &ComputeTarget,
    runner: &Runner,
    root: &str,
) -> Result<String, DeployError> {
    if let Some(text) = host_channel::remote_json_member(
        target,
        &format!("{root}/package.json"),
        &["version"],
        runner,
    )
    .await?
    {
        if let Some(version) = host_channel::extract_semver(&text) {
            return Ok(version);
        }
    }
    if let Some(stamp) =
        host_channel::remote_read_file(target, &format!("{root}/.weles-release"), runner).await?
    {
        // `version=` when the stamp carries one, otherwise the version
        // segment of the immutable coordinate the artefact was fetched from:
        //   release_uri=stado://releases/<product>/<version>/<platform>/<archive>
        let stamped = stamp
            .lines()
            .find_map(|line| line.strip_prefix("version="))
            .filter(|value| !value.is_empty())
            .or_else(|| {
                stamp.lines().find_map(|line| {
                    let rest = line.strip_prefix("release_uri=stado://releases/")?;
                    let mut segments = rest.split('/');
                    segments.next()?;
                    let version = segments.next()?;
                    segments.next().map(|_| version)
                })
            });
        if let Some(version) = stamped.and_then(host_channel::extract_semver) {
            return Ok(version);
        }
    }
    for keys in [
        &["version"][..],
        &["buildDefinition", "externalParameters", "tag"][..],
    ] {
        if let Some(text) = host_channel::remote_json_member(
            target,
            &format!("{root}/provenance.json"),
            keys,
            runner,
        )
        .await?
        {
            if let Some(version) = host_channel::extract_semver(&text) {
                return Ok(version);
            }
        }
    }
    Ok(String::new())
}

/// Report, for every binary this host has a declared `managed_versions` entry
/// for, which version it is actually running — natively, one remote command
/// per question, with the report text composed here in the wire format
/// [`parse_report`] reads.
///
/// The declaration is the scope — a binary nobody declared is not this
/// reporter's business, and reporting it would bury the ones that are. Where
/// an installed version comes from, in this order, first hit wins:
///
///   1. `$HOME/.stado/bin/<name>` — an owner-only Stado program, asked
///      directly;
///   2. package.json `version`;
///   3. `.weles-release`;
///   4. `provenance.json` — `.version` when it carries one, otherwise the
///      build's own tag.
///
/// A product whose artefact carries none of those reports `version=unknown`.
/// That is the honest answer and it is never rounded to the declared version:
/// `service converge` reports it as `unknown`, never as `in-sync`.
///
/// Read-only, and strictly so: nothing is fetched, nothing is written, no
/// unit is restarted, and no credential is printed — the only values emitted
/// are binary names, versions, paths, unit labels and launchd state.
/// Whether the installed artefact is the copy the delivery path staged for
/// the version it claims.
///
/// Host-local and cheap: `cmp -s` against
/// `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, which
/// `host release` writes and verifies against the canonical manifest's
/// SHA-256 before it installs anything. No network, no manifest fetch, and no
/// second opinion needed about what a version string means — a local build
/// claiming a released version has no staged copy to match.
async fn attest_installed(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    binary: &str,
    root: &str,
    version: &str,
) -> Result<(&'static str, String), DeployError> {
    if version.is_empty() || root.is_empty() || !host_release::is_exact_semver(version) {
        return Ok((ATTEST_UNKNOWN, String::new()));
    }
    let platform = target.release_platform.trim();
    if platform.is_empty() {
        return Ok((ATTEST_UNKNOWN, String::new()));
    }
    let coordinate = format!("{home}/.stado/releases/{binary}/{version}/{platform}");
    let staged = format!("{coordinate}/{binary}");
    let quoted_staged = crate::deploy::shlex_quote(&staged);
    if !host_channel::remote_test(target, &format!("-f {quoted_staged}"), runner).await? {
        // `host release` creates `<binary>/<version>/<platform>/` only when it
        // stages, so the binary directory existing at all is the record that
        // this host has been delivered to before. Its absence is bootstrap,
        // not tampering.
        let history = format!("{home}/.stado/releases/{binary}");
        let quoted_history = crate::deploy::shlex_quote(&history);
        if host_channel::remote_test(target, &format!("-d {quoted_history}"), runner).await? {
            return Ok((ATTEST_ABSENT, String::new()));
        }
        return Ok((ATTEST_NEVER_DELIVERED, String::new()));
    }
    // A byte comparison, not a digest: the two files are already on the same
    // disk, `cmp -s` reads no further than the first difference, and there is
    // no hash to agree on between this process and the host.
    let same = host_channel::run_command(
        target,
        &format!(
            "/usr/bin/cmp -s {} {quoted_staged}",
            crate::deploy::shlex_quote(root)
        ),
        runner,
    )
    .await?
    .ok();
    if !same {
        return Ok((ATTEST_DIFFERS, String::new()));
    }
    // The receipt `host release` leaves beside the staged copy, when there is
    // one. A delivery made before that format has none, and its absence is
    // "installed before receipts" rather than a finding: the byte comparison
    // above has already attested these bytes without it.
    let receipt = host_channel::run_command(
        target,
        &format!(
            "/bin/cat {}/release-receipt.json 2>/dev/null || true",
            crate::deploy::shlex_quote(&coordinate)
        ),
        runner,
    )
    .await?;
    let summary = serde_json::from_str::<Value>(receipt.stdout.trim())
        .ok()
        .map(|document| {
            let field = |key: &str| {
                document
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            (field("installed_at"), field("delivered_by"))
        })
        .filter(|(at, by)| !at.is_empty() || !by.is_empty())
        .map(|(at, by)| format!("delivered {at} by {by}"))
        .unwrap_or_default();
    Ok((ATTEST_MATCH, summary))
}

async fn probe_installed_versions(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let services = declared_service_records(target);
    let mut uid = None;
    let mut out = format!(
        "# host {}  registry canonical  at {}\n",
        target.name,
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    let mut count = 0usize;
    for binary in target.managed_versions.keys() {
        count += 1;

        let stado_program = format!("{home}/.stado/bin/{binary}");
        let quoted_program = crate::deploy::shlex_quote(&stado_program);
        let (root, version) =
            if host_channel::remote_test(target, &format!("-x {quoted_program}"), runner).await?
                && host_channel::remote_test(target, &format!("-f {quoted_program}"), runner)
                    .await?
            {
                let version = host_channel::remote_program_version(target, &stado_program, runner)
                    .await?
                    .unwrap_or_default();
                (stado_program, version)
            } else {
                let root = artefact_root(target, runner, &home, binary).await?;
                let version = if root.is_empty() {
                    String::new()
                } else {
                    artefact_version(target, runner, &root).await?
                };
                (root, version)
            };

        // Provenance, asked host-locally: the delivery path stages every
        // release it installs at
        // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`,
        // digest-verified against the canonical manifest on the way in. If the
        // version these bytes claim has no staged copy, or the installed file
        // is not that copy, the bytes did not come through delivery.
        let (attestation, receipt) =
            attest_installed(target, runner, &home, binary, &root, &version).await?;

        let mut unit = "none".to_string();
        let mut state = "none".to_string();
        if let Some((label, path, kind)) =
            unit_for_root(target, runner, &home, &services, &root).await?
        {
            state = unit_state(target, runner, &label, &path, &kind, &mut uid).await?;
            unit = label;
        }

        out.push_str(&format!(
            "binary={} version={} root={} unit={} state={} attestation={} receipt={}\n",
            binary,
            if version.is_empty() {
                UNKNOWN
            } else {
                &version
            },
            if root.is_empty() { "none" } else { &root },
            unit,
            state,
            attestation,
            // One token, so the line stays `key=value` and a receipt with a
            // space in it cannot become two fields.
            if receipt.is_empty() {
                NONE.to_string()
            } else {
                receipt.replace(' ', "_")
            },
        ));
    }
    out.push_str(&format!("# binaries {count}\n"));
    Ok(out)
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
            } else if let Some(value) = token.strip_prefix("attestation=") {
                entry.attestation = value.to_string();
            } else if let Some(value) = token.strip_prefix("receipt=") {
                entry.receipt = if value == NONE {
                    String::new()
                } else {
                    value.replace('_', " ")
                };
            }
        }
        let Some(binary) = binary else {
            continue;
        };
        if host_release::is_exact_semver(raw_version) {
            entry.version = Some(raw_version.to_string());
        }
        if entry.attestation.is_empty() {
            entry.attestation = ATTEST_UNKNOWN.to_string();
        }
        reported.insert(binary.to_string(), entry);
    }
    reported
}

/// Direction-aware ordering of two exact semantic versions.
///
/// The numeric core decides, per semver; equal cores are settled by the
/// prerelease the same way: a release outranks its own prereleases, numeric
/// identifiers order numerically and below alphanumeric ones, alphanumeric
/// ones lexically, and a longer list outranks its own prefix. Both inputs
/// here have already passed [`host_release::is_exact_semver`], so the parse
/// cannot fail; the `Option` is the parse's own honesty, not a third answer.
fn version_order(left: &str, right: &str) -> Option<Ordering> {
    fn core(version: &str) -> Option<(u64, u64, u64)> {
        let core = version.split_once('-').map_or(version, |(core, _)| core);
        let mut parts = core.split('.');
        let triple = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        if parts.next().is_some() {
            return None;
        }
        Some(triple)
    }
    fn prerelease(version: &str) -> &str {
        version
            .split_once('-')
            .map_or("", |(_, prerelease)| prerelease)
    }
    fn prerelease_order(left: &str, right: &str) -> Ordering {
        match (left.is_empty(), right.is_empty()) {
            (true, true) => return Ordering::Equal,
            // A release outranks every one of its own prereleases.
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        let mut lefts = left.split('.');
        let mut rights = right.split('.');
        loop {
            let ordering = match (lefts.next(), rights.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(left), Some(right)) => {
                    let numeric = |identifier: &str| identifier.bytes().all(|b| b.is_ascii_digit());
                    match (numeric(left), numeric(right)) {
                        (true, true) => left
                            .parse::<u64>()
                            .unwrap_or(u64::MAX)
                            .cmp(&right.parse::<u64>().unwrap_or(u64::MAX)),
                        // Numeric identifiers order below alphanumeric ones.
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        (false, false) => left.cmp(right),
                    }
                }
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
    }
    let ordering = core(left)?.cmp(&core(right)?);
    if ordering != Ordering::Equal {
        return Some(ordering);
    }
    Some(prerelease_order(prerelease(left), prerelease(right)))
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
            let attestation = entry
                .map(|entry| entry.attestation.as_str())
                .unwrap_or(ATTEST_UNKNOWN);
            // Provenance is judged before drift, because drift between a
            // declaration and bytes nobody delivered is not the finding: the
            // bytes are. Reading these as `host-ahead` is what let a local
            // build offer to promote its own version into the registry.
            let unattested = match attestation {
                ATTEST_ABSENT => Some(format!(
                    "the host runs {} and no delivered copy of {} is staged at \
                     $HOME/.stado/releases, though earlier versions of this binary were \
                     delivered here; these bytes were put at the install path beside the \
                     delivery path, and --apply will not move the declaration to a version \
                     it cannot attest",
                    installed.as_deref().unwrap_or(UNKNOWN),
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                ATTEST_NEVER_DELIVERED => Some(format!(
                    "the host runs {} and this binary has never been delivered here: \
                     $HOME/.stado/releases holds no version of it at all. The bootstrap \
                     installer stages nothing, so this is the expected reading for a host \
                     that has not had a `stado host release` yet — it is not evidence that \
                     anything was replaced",
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                ATTEST_DIFFERS => Some(format!(
                    "the host runs {} and the staged copy of {} does not match the installed \
                     file byte for byte; the binary was replaced after delivery",
                    installed.as_deref().unwrap_or(UNKNOWN),
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                _ => None,
            };
            if let Some(detail) = unattested {
                return Row {
                    binary: binary.clone(),
                    declared: declared_version.clone(),
                    installed,
                    verdict: UNATTESTED,
                    detail,
                    root: entry.map(|entry| entry.root.clone()).unwrap_or_default(),
                    unit: entry.map(|entry| entry.unit.clone()).unwrap_or_default(),
                    state: entry.map(|entry| entry.state.clone()).unwrap_or_default(),
                    running_binary: None,
                    binary_matches_process: None,
                };
            }
            let (verdict, detail) = match (&installed, reported) {
                (Some(version), _) if version == declared_version => (
                    IN_SYNC,
                    // Provenance, when the host kept a record of it. "-" still
                    // means attested-by-bytes with no receipt, which is every
                    // delivery made before the receipt format.
                    entry
                        .map(|entry| entry.receipt.clone())
                        .filter(|receipt| !receipt.is_empty())
                        .unwrap_or_else(|| String::from("-")),
                ),
                (Some(version), _) => match version_order(version, declared_version) {
                    // The host is behind the declaration: `--apply` delivers
                    // the declared one, which is an upgrade here.
                    Some(Ordering::Less) => (
                        HOST_BEHIND,
                        format!(
                            "the host runs {version}, older than the declared \
                             {declared_version}; --apply delivers the declared one \
                             through `stado host release`"
                        ),
                    ),
                    // The declaration is behind the host: delivering it would
                    // be a downgrade, so nothing is delivered and the remedy
                    // is to move the declaration.
                    Some(Ordering::Greater) => (
                        HOST_AHEAD,
                        format!(
                            "the host runs {version}, newer than the declared \
                             {declared_version}: the declaration is stale, not the \
                             host; --apply refuses to downgrade it and names the \
                             declare-version command that moves the declaration"
                        ),
                    ),
                    // Equal orderings of unequal strings cannot happen for two
                    // exact semantic versions, and are reported as in sync
                    // rather than invented into drift if one ever does.
                    Some(Ordering::Equal) => (IN_SYNC, String::from("-")),
                    // Unreachable for two exact semantic versions; reported
                    // unmeasured rather than ordered by invention.
                    None => (
                        UNKNOWN,
                        format!(
                            "the host runs {version} against the declared \
                             {declared_version}, and the two cannot be ordered"
                        ),
                    ),
                },
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

/// Deliver the declared version of every binary that is behind it or running
/// bytes the fleet cannot attest, and refuse only what a delivery would take
/// backwards.
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
///
/// `host-ahead` rows are refused outright: the host runs NEWER than the
/// declaration, so delivering the declared version is a downgrade of a live
/// host, and a converge that performs one is the registry's staleness shipped
/// as an outage. Each refusal records the exact `stado host declare-version`
/// command that moves the declaration to the observed version instead.
async fn apply_releases(target: &str, rows: &[Row], runner: &Runner) -> AppliedPass {
    let mut pass = AppliedPass::default();
    // An unattested binary is delivered, not reported at.
    //
    // Until 2026-09-02 every `unattested` row was refused with a remediation
    // naming `stado host release TARGET --binary X --version Y` — which is
    // `host_release::release_host`, the function this very command calls three
    // lines further down for a host that is merely behind. So `--apply` printed
    // the command it owns and declined to run it, and the 0.13.46 train died on
    // that twice: `deploy-fleet` declared 0.13.46 for charless-mac-mini, found
    // it running an unattested 0.13.45, refused, and exited non-zero, while the
    // delivery that fixed it arrived minutes later from the host's own release
    // agent. Bytes with no provenance are exactly what a delivery replaces.
    //
    // The one case still refused is a host strictly AHEAD of its declaration:
    // there a delivery takes a live host backwards on a stale declaration, and
    // the remediation stays a delivery rather than `declare-version`, because
    // writing an unattested version into the registry is the failure, not the
    // fix.
    for row in rows.iter().filter(|row| row.verdict == UNATTESTED) {
        let ahead = row
            .installed
            .as_deref()
            .and_then(|installed| version_order(installed, &row.declared))
            == Some(Ordering::Greater);
        if ahead {
            pass.refused.push(Refused {
                binary: row.binary.clone(),
                declared: row.declared.clone(),
                installed: row.installed_cell().to_string(),
                remediation: format!(
                    "stado host release {target} --binary {} --version {} (deliver a published \
                     version; do NOT declare-version onto bytes the fleet cannot attest)",
                    row.binary, row.declared
                ),
            });
            continue;
        }
        eprintln!(
            "{}: runs {}, which this fleet cannot attest: {}",
            row.binary,
            row.installed_cell(),
            row.detail
        );
        deliver(target, row, runner, &mut pass).await;
    }
    for row in rows.iter().filter(|row| row.verdict == HOST_AHEAD) {
        let remediation = format!(
            "stado host declare-version {target} --binary {} --version {}",
            row.binary,
            row.installed_cell()
        );
        pass.refused.push(Refused {
            binary: row.binary.clone(),
            declared: row.declared.clone(),
            installed: row.installed_cell().to_string(),
            remediation,
        });
    }
    for row in rows.iter().filter(|row| row.verdict == HOST_BEHIND) {
        eprintln!(
            "{}: declared {} but runs {}",
            row.binary,
            row.declared,
            row.installed_cell()
        );
        deliver(target, row, runner, &mut pass).await;
    }
    pass
}

/// Finish the runtime half even when the installed Stado file was already
/// attested and at the declared version.
///
/// `install-local` may have replaced the root file and then failed while
/// recycling one reader. A resumed `service converge --apply` must not read the
/// matching file as completion and skip the still-old process. The target's
/// installed binary owns the same kernel-identity implementation used during
/// install, so this invokes that implementation rather than redelivering bytes.
async fn converge_native_readers(
    target: &ComputeTarget,
    declared: &[(String, String)],
    runner: &Runner,
    pass: &mut AppliedPass,
) {
    let version = declared
        .iter()
        .find(|(name, _)| name == "stado")
        .map(|(_, version)| version.clone())
        .unwrap_or_default();
    let script = "set -euo pipefail\n\"$HOME/.stado/bin/stado\" release \
                  converge-local-readers --name stado\n";
    let outcome = host_channel::run_script(target, script, runner).await;
    let (status, detail) = match outcome {
        Ok(output) if output.ok() => (COMPLETED, output.stdout.trim().to_string()),
        Ok(output) => (
            FAILED,
            host_channel::last_error_line(&output, "native reader convergence failed"),
        ),
        Err(error) => (FAILED, error.to_string()),
    };
    pass.releases.push(Released {
        binary: "stado-readers".to_string(),
        version,
        status,
        detail,
    });
}

/// One delivery, recorded. The only call site of
/// [`host_release::release_host`] in this command, so a host that is behind and
/// a host whose bytes cannot be attested are converged by the same path and
/// cannot drift apart in what "delivered" means.
async fn deliver(target: &str, row: &Row, runner: &Runner, pass: &mut AppliedPass) {
    if let Err(error) = crate::deploy::products::product(&row.binary) {
        pass.undeliverable.push(Undeliverable {
            binary: row.binary.clone(),
            detail: error.to_string(),
        });
        return;
    }
    eprintln!("{target}: releasing {} {}", row.binary, row.declared);
    match host_release::release_host(target, &row.binary, &row.declared, false, false, runner).await
    {
        Ok(report) => {
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let delivered = matches!(
                status,
                host_release::RELEASED_STATUS | host_release::ALREADY_ACTIVE_STATUS
            );
            // `host release` reports host-side refusals as a structured
            // `Ok(report)`, with any diagnostic in `error`. Reducing that
            // report to its status discarded the only place a cause could be
            // retained: historical trains printed only `detail: "failed"`,
            // which does not establish whether the inner report had an error.
            let detail = if delivered {
                status.to_string()
            } else {
                report
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|detail| !detail.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        if status.is_empty() {
                            String::from("the delivery reported neither a status nor an error")
                        } else {
                            format!("delivery returned non-success status {status} without an error")
                        }
                    })
            };
            pass.releases.push(Released {
                binary: row.binary.clone(),
                version: row.declared.clone(),
                status: if delivered { COMPLETED } else { FAILED },
                detail,
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
                "refused": pass.refused.iter().map(Refused::to_json).collect::<Vec<Value>>(),
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
    //
    // This used to end "restart it to pick up what is installed", which is the
    // wrong instruction to hand someone at seven in the morning. A stale
    // process is a fact, not a fault. On 2026-08-31 `com.wisent.stado-resolver`
    // on charless-mac-mini reported this line after a clean 0.13.9 delivery,
    // and cycling it would have been tidiness: the running binary had no
    // functional symptom, and restarting a load-bearing resolver to silence a
    // diff is how a degraded host becomes a down host. So the line now states
    // the condition under which the restart is actually required, and leaves
    // the judgement where it belongs.
    for row in rows
        .iter()
        .filter(|row| row.process_cell() == PROCESS_DIFFERS)
    {
        eprintln!(
            "{}: the process under {} is running {} — not the artefact this \
             unit's declaration resolves to. A stale process is not itself a \
             fault. Restart it when the running binary lacks behaviour you now \
             need — it cannot parse a registry value a newer version added, or \
             a fix that this process executes has been delivered — and not \
             merely because this line is printed.",
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
    for entry in &pass.refused {
        eprintln!(
            "{}: runs {}, newer than the declared {} — refused to downgrade the \
             host; move the declaration instead: {}",
            entry.binary, entry.installed, entry.declared, entry.remediation
        );
    }
    Ok(())
}

/// Report mode: drift in either direction fails, an unmeasured binary does
/// not.
///
/// This is what makes the command usable as a gate. A host behind or ahead of
/// its declaration is a false declaration and belongs in a non-zero exit; a
/// host whose reporter is not installed, or a product whose artefact carries
/// no version metadata, has produced no evidence either way, and turning that
/// into a failure teaches operators to pass `|| true`, at which point the
/// drift the command exists to catch stops being noticed again. Every such
/// row is named on stderr instead, because the one thing an unmeasured
/// product must never be is quiet.
fn report_gate(rows: &[Row]) -> Result<(), CmdError> {
    for row in rows.iter().filter(|row| row.verdict == UNKNOWN) {
        eprintln!(
            "{}: declared {} and no installed version could be read — unmeasured, \
             not in sync: {}",
            row.binary, row.declared, row.detail
        );
    }
    let behind = rows.iter().filter(|row| row.verdict == HOST_BEHIND).count();
    let ahead = rows.iter().filter(|row| row.verdict == HOST_AHEAD).count();
    let unattested = rows.iter().filter(|row| row.verdict == UNATTESTED).count();
    if behind + ahead + unattested == 0 {
        return Ok(());
    }
    // Named first and loudest: a version the fleet cannot attest outranks a
    // version it can attest and disagrees with.
    if unattested != 0 {
        eprintln!(
            "{unattested} declared binary/binaries run bytes this fleet cannot attest: \
             the version they claim has no delivered copy staged on the host, or the \
             installed file is not the one that was staged. A version string is not \
             provenance. Deliver a published version with `stado host release`; do not \
             move the declaration onto them"
        );
    }
    if behind != 0 {
        eprintln!(
            "{behind} declared binary/binaries run a version older than the \
             registry declares; re-run with --apply to deliver the declared one"
        );
    }
    if ahead != 0 {
        eprintln!(
            "{ahead} declared binary/binaries run a version NEWER than the \
             registry declares: the declaration is stale, not the host; \
             `stado host declare-version` moves it, --apply will not touch \
             these hosts"
        );
    }
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
    let failed = pass
        .releases
        .iter()
        .filter(|entry| entry.status == FAILED)
        .count();
    if unresolved.is_empty()
        && failed == 0
        && pass.undeliverable.is_empty()
        && pass.refused.is_empty()
    {
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
    // A failed delivery remains a failed apply even when the final read finds
    // matching bytes (for example because another release actor converged the
    // host concurrently). The delivery receipt is an asserted part of this
    // operation, not disposable progress text.
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
        if pass.releases.is_empty() && pass.refused.is_empty() {
            effort.push_str(", because nothing was confirmed behind its declaration");
        }
    } else {
        effort.push_str(&format!(
            "; {} host-behind binary/binaries are not deliverable by `stado host release`",
            pass.undeliverable.len()
        ));
    }
    // Every refusal used to be reported as `host-ahead`, whatever it was. On
    // 2026-09-02 charless-mac-mini was BEHIND its declaration — 0.13.45 against
    // 0.13.46 — and the summary told the release train that a host ahead of the
    // registry had been refused rather than downgraded, which is the opposite
    // diagnosis and points an operator at `declare-version` when the answer was
    // a delivery. A refusal is classified by the row it came from.
    if !pass.refused.is_empty() {
        let kind = |verdict: &str| {
            pass.refused
                .iter()
                .filter(|entry| {
                    rows.iter()
                        .any(|row| row.binary == entry.binary && row.verdict == verdict)
                })
                .count()
        };
        let ahead = kind(HOST_AHEAD);
        let unattested = kind(UNATTESTED);
        if ahead != 0 {
            effort.push_str(&format!(
                "; {ahead} host-ahead binary/binaries were refused rather than downgraded — \
                 the declaration is stale, not the host"
            ));
        }
        if unattested != 0 {
            effort.push_str(&format!(
                "; {unattested} binary/binaries run unattested bytes NEWER than the \
                 declaration, so no delivery was made — move the declaration to a published \
                 version first"
            ));
        }
    }
    eprintln!(
        "{} binary/binaries are not at their declared version after {effort}",
        unresolved.len()
    );
    Err(CmdError::silent(CLICK_ERROR_CODE))
}

//! Standing checks for the shape of the fleet: is what is declared what is
//! running, and does anything measure the difference.
//!
//! NO Python original. Written on 2026-08-31 after a night in which seven
//! defects of ONE shape were fixed by hand and nothing in the product would
//! have caught the eighth. Every check here is a question somebody had to ask
//! a host by hand that night, and the answer each time was a surprise:
//!
//! - three processes served one declared port on `charless-mac-mini`
//!   (`127.0.0.1:8765`, `[::1]:8765`, and a `node` on the tailnet address),
//!   found with `lsof` after hours of treating the symptom as a slow link;
//! - a label declared in two launchd domains ran twice and was invisible to
//!   `service list --undeclared`, precisely BECAUSE the label was declared;
//! - the live object API answered `healthz` 200 while every object route
//!   returned 503, so the health check was green on a server refusing its
//!   entire purpose;
//! - a primary addressed by bare key with a replica addressed by qualified
//!   path silently produced 48 GiB of objects nothing could resolve;
//! - a managed host declared two cleaners, neither of which could reach what
//!   actually filled its disk, and nothing said so.
//!
//! The rule this module exists to enforce on itself: **a check that cannot
//! fail is the disease.** So every check reports what it MEASURED, and a check
//! that could not measure its subject says that in a finding rather than
//! passing quietly. `measured` on [`Sweep`] is the count of subjects actually
//! interrogated, and a sweep that measured nothing is not a clean sweep.
//!
//! Each finding names four things, because a verdict without them is what made
//! these take hours: the SUBJECT it is about, what the fleet DECLARES, what was
//! OBSERVED, and the exact COMMAND that resolves it.
//!
//! Two entry points, one implementation: [`sweep`] is called by
//! [`crate::doctor`] for an operator asking now, and by
//! [`crate::coordinator`]'s tick so nobody has to ask. The tick is the reason
//! this is not another command nobody runs.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::{host_channel, host_disk, service, Runner};
use crate::queue::copy::Endpoint;
use crate::targets::{ComputeTarget, Registry};

/// How many subjects one rule interrogated on one host.
///
/// The prose note beside it says the same thing in a sentence an operator
/// reads. This is the same number in a field something else can consume: a
/// count nobody can query is a count nobody can trend, gate or alert on, and
/// on 2026-09-03 answering "did the prefix rule actually look at anything"
/// meant parsing a 54,894-character string by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    /// Stable id of the rule that did the interrogating, or of the check
    /// itself when the count is not per-rule.
    pub check: &'static str,
    /// The host the subjects were counted on, when the count is per-host.
    pub host: Option<String>,
    /// Subjects actually interrogated. Zero means this rule proved nothing
    /// here, which is a result and not a silence.
    pub subjects: u64,
}

impl Measurement {
    pub fn new(check: &'static str, host: Option<String>, subjects: u64) -> Self {
        Self {
            check,
            host,
            subjects,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "check": self.check,
            "host": self.host,
            "subjects": self.subjects,
            "proved_nothing": self.subjects == 0,
        })
    }
}

/// One thing that is not the way the fleet says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable id of the check that produced it, for grepping a tick log.
    pub check: &'static str,
    /// What the finding is about: a host, a label, a port, a store.
    pub subject: String,
    /// What the fleet declares about that subject.
    pub declared: String,
    /// What was actually observed.
    pub observed: String,
    /// The exact command that resolves it.
    pub command: String,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({
            "check": self.check,
            "subject": self.subject,
            "declared": self.declared,
            "observed": self.observed,
            "command": self.command,
        })
    }

    /// One line carrying all four parts. The tick log is the only place some
    /// of these will ever be read, so the line has to be the whole finding.
    pub fn line(&self) -> String {
        format!(
            "{}: {} — declared {} — observed {} — fix: {}",
            self.check, self.subject, self.declared, self.observed, self.command
        )
    }
}

/// What one sweep looked at and what it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub findings: Vec<Finding>,
    /// Subjects actually interrogated. A sweep with zero of these has proven
    /// nothing, and says so instead of reading as healthy.
    pub measured: u32,
    /// Hosts the sweep could not reach at all, by name and reason.
    pub unreachable: Vec<(String, String)>,
    /// What a check measured when it had nothing to report. Present so that a
    /// silent check and a check with nothing to check are distinguishable.
    pub notes: Vec<String>,
    /// The same per-rule counts the notes carry in prose, as fields.
    pub measurements: Vec<Measurement>,
}

impl Sweep {
    fn record(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn summary(&self) -> String {
        if self.measured == 0 {
            return format!(
                "fleet shape: NOTHING measured ({} host(s) unreachable), so this is not a clean result",
                self.unreachable.len()
            );
        }
        format!(
            "fleet shape: {} subject(s) measured, {} finding(s), {} host(s) unreachable{}",
            self.measured,
            self.findings.len(),
            self.unreachable.len(),
            if self.notes.is_empty() {
                String::new()
            } else {
                format!(" — measured clean: {}", self.notes.join(", "))
            }
        )
    }
}

/// Per-host wall clock. A host that has gone quiet must cost one line, not the tick. Three remote
/// tick it was swept from.
const HOST_TIMEOUT: Duration = Duration::from_secs(240);

pub const PORT_CHECK: &str = "one-listener-per-declared-port";
pub const DOMAIN_CHECK: &str = "one-domain-per-declared-label";
pub const HEALTH_CHECK: &str = "health-green-boundaries-down";
pub const REPLICA_CHECK: &str = "replica-cannot-resolve";
pub const DISK_CHECK: &str = "disk-headroom-against-policy";
pub const PROGRAM_CHECK: &str = "loaded-label-runs-declared-program";
pub const BINARY_CHECK: &str = "loaded-label-runs-installed-binary";
pub const ARTEFACT_CHECK: &str = "service-artefact-not-older-than-installed";
pub const PREFIX_CHECK: &str = "label-carries-its-prefix-once";
pub const ORPHAN_CHECK: &str = "loaded-job-has-a-unit-file";
pub const RESTART_CHECK: &str = "job-runs-are-work-not-a-loop";
pub const SHADOW_CHECK: &str = "path-resolves-the-delivered-binary";
pub const UNIT_ENV_CHECK: &str = "unit-declares-the-environment-its-program-reads";

/// The label prefix this fleet's own installer mints.
///
/// A label that carries it twice was built by applying it to a name that
/// already had it. That is not cosmetic: the doubled label is a DIFFERENT
/// label, so it is declared nowhere, every ownership reader calls it
/// undeclared, and launchd runs it anyway.
const MINTED_PREFIX: &str = "com.wisent.compute.service.";

/// Variables launchd hands every job, so a script reading one of these is not
/// reading something its unit file failed to declare.
///
/// Deliberately short. Anything not on it that a script reads and does not
/// default is a variable somebody has to supply, and the unit file is the only
/// thing that can.
const AMBIENT_VARIABLES: [&str; 12] = [
    "HOME", "PATH", "USER", "SHELL", "TMPDIR", "PWD", "OLDPWD", "LANG", "LC_ALL", "IFS", "UID",
    "LOGNAME",
];

/// Runs past which a job is not doing work on a schedule, it is looping.
///
/// `stado-resolver` was at 50,863 and `claude-reauth-once` at 45,418 while
/// every other reader called the host healthy, so the threshold only has to be
/// far enough above a legitimate restart count to be beyond argument.
const RESTART_LOOP_RUNS: u64 = 500;

/// Sweep the whole canonical registry.
///
/// Never returns an error: an unreachable host is a recorded fact, because the
/// useful output is the whole list and one dead box must not suppress it.
pub async fn sweep(runner: &Runner) -> Sweep {
    let mut result = Sweep::default();
    let registry = match host_channel::canonical_registry().await {
        Ok(registry) => registry,
        Err(error) => {
            result
                .unreachable
                .push(("<registry>".to_string(), error.to_string()));
            return result;
        }
    };
    // The store-addressing check is answered from configuration alone, so it
    // runs even when every host is unreachable.
    replica_addressing(&mut result);
    for target in registry.targets.iter().filter(|target| target.slots > 0) {
        sweep_host(&registry, target, runner, &mut result).await;
    }
    result
}

/// Everything one host is asked, under one deadline.
async fn sweep_host(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    result: &mut Sweep,
) {
    match tokio::time::timeout(HOST_TIMEOUT, host_findings(registry, target, runner)).await {
        Ok(Ok((mut findings, mut notes, mut measurements))) => {
            result.measured += 1;
            for finding in findings.drain(..) {
                result.record(finding);
            }
            result.notes.append(&mut notes);
            result.measurements.append(&mut measurements);
        }
        Ok(Err(error)) => result.unreachable.push((target.name.clone(), error)),
        Err(_) => result.unreachable.push((
            target.name.clone(),
            format!("did not answer within {}s", HOST_TIMEOUT.as_secs()),
        )),
    }
}

/// What one host answered: things to act on, and things measured that need no
/// action. Both are returned, because a check that stays silent when it found
/// nothing to report is indistinguishable from one that never ran.
async fn host_findings(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<(Vec<Finding>, Vec<String>, Vec<Measurement>), String> {
    let mut findings = Vec::new();
    let mut notes = Vec::new();
    let (loaded, posture) = service::loaded_units_with_posture(target, runner)
        .await
        .map_err(|error| error.to_string())?;
    duplicate_domains(target, &loaded, &mut findings);
    process_identity(target, &loaded, &mut findings, &mut notes);
    // The five checks added on 2026-09-02, each one a state that was found by
    // hand during the #286 hunt and that nothing would have reported again.
    let mut labels_measured = 0_usize;
    let mut runs_measured = 0_usize;
    let mut env_measured = 0_usize;
    let mut path_measured = 0_usize;
    doubled_prefix(target, &loaded, &mut findings, &mut labels_measured);
    let mut orphan_measured = 0_usize;
    loaded_without_unit_file(target, &loaded, &mut findings, &mut orphan_measured);
    restart_loops(target, &loaded, &mut findings, &mut runs_measured);
    unit_environment(target, &loaded, &mut findings, &mut env_measured);
    path_binary(
        target,
        posture.as_ref(),
        &mut findings,
        &mut notes,
        &mut path_measured,
    );
    // A check that cannot say what it looked at cannot be trusted when it is
    // quiet, which is the rule this module holds itself to. One note per
    // check, each naming the check's own id, because the single prose sentence
    // this replaced named none of them: "983 label(s) read for a doubled
    // prefix" cannot be matched to `label-carries-its-prefix-once` by anything
    // but a human who already knows the code, so "how many subjects did this
    // check interrogate" was unanswerable from `doctor --json` even though the
    // number was right there.
    let mut measurements = Vec::new();
    for (check, measured) in [
        (PREFIX_CHECK, labels_measured),
        (ORPHAN_CHECK, orphan_measured),
        (RESTART_CHECK, runs_measured),
        (UNIT_ENV_CHECK, env_measured),
        (SHADOW_CHECK, path_measured),
    ] {
        notes.push(format!(
            "measured {check} on {}: {measured} subject(s){}",
            target.name,
            if measured == 0 {
                " — ZERO, so this check proved nothing here"
            } else {
                ""
            }
        ));
        measurements.push(Measurement::new(
            check,
            Some(target.name.clone()),
            measured as u64,
        ));
    }
    // One inventory read, two questions: which processes hold which declared
    // ports, and whether the artefacts behind the service units are the ones
    // the fleet has installed.
    if let Some(reading) = listener_count(registry, target, runner, &mut findings).await {
        service_artefacts(target, &reading, &mut findings, &mut notes);
    }
    disk_headroom(target, runner, &mut findings, &mut notes).await;
    Ok((findings, notes, measurements))
}

/// A label carries this fleet's minted prefix exactly once.
///
/// `label()` used to prefix a name that already carried the prefix, and the
/// function was fixed early. Nobody went looking for the jobs it had already
/// minted, and on 2026-09-01 eight of them were loaded on charless-mac-mini —
/// one of them a system LaunchDaemon with `KeepAlive` running
/// `stado agent --target charless-mac-mini`, which recreated an undeclared
/// queue agent for days. Three separate sessions hunted it as a rogue script.
///
/// It is a string comparison. It would have ended that hunt on the first
/// sweep, and it is here because the absence of this one line cost more than
/// every check above it put together.
fn doubled_prefix(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        *measured += 1;
        let Some(rest) = unit.label.strip_prefix(MINTED_PREFIX) else {
            continue;
        };
        if !rest.starts_with(MINTED_PREFIX) && !rest.starts_with("com.wisent.") {
            continue;
        }
        out.push(Finding {
            check: PREFIX_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: format!("one {MINTED_PREFIX} prefix per label"),
            observed: format!(
                "the label already carried a fleet prefix, so it was minted onto one: the real name is {rest}"
            ),
            command: format!(
                "stado service label-print {} --host {} to see what it holds, then stado service bootout {} --host {} --domain <the domain it is loaded in>",
                unit.label, target.name, unit.label, target.name
            ),
        });
    }
}

/// A job launchd holds has a unit file somebody can read.
///
/// launchd keeps a job after its plist is deleted, and every reader in this
/// binary enumerated unit files or one launchd domain. A job loaded in the
/// system domain with no file on disk was therefore in nobody's list, and that
/// is exactly where the #286 respawner lived: loaded, restarting on
/// `KeepAlive`, invisible, and reported by `list --undeclared`,
/// `list --unowned` and the reap keep-set as "no label holds this".
///
/// The population is the jobs whose PROGRAM comes out of a fleet-managed root
/// — evidence in hand, not the label's spelling. Every `application.com.apple.*`
/// row on a mac is loaded with no unit file too, and it is not this fleet's
/// business.
fn loaded_without_unit_file(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.loaded_domains.is_empty() || !unit.declaring_paths.is_empty() {
            continue;
        }
        *measured += 1;
        let program = if unit.running_program.is_empty() {
            unit.program.as_str()
        } else {
            unit.running_program.as_str()
        };
        if !fleet_program(program) {
            continue;
        }
        out.push(Finding {
            check: ORPHAN_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "a loaded job has a unit file in one of the three fleet directories"
                .to_string(),
            observed: format!(
                "launchd holds it in {} with no unit file on disk{}, running {program}",
                unit.loaded_domains.join(", "),
                if unit.path.is_empty() {
                    String::new()
                } else {
                    format!(" (loaded from {}, now gone)", unit.path)
                }
            ),
            command: format!(
                "stado service bootout {} --host {} --domain {}",
                unit.label,
                target.name,
                unit.loaded_domains
                    .first()
                    .map_or("system", |domain| domain.as_str())
            ),
        });
    }
}

/// Does a program come out of a root this fleet installs into?
///
/// Asked of the path rather than of a label, because the label is the thing
/// that lied in every incident this module records.
fn fleet_program(program: &str) -> bool {
    let first = program
        .split_whitespace()
        .find(|word| word.starts_with('/'));
    let Some(path) = first else { return false };
    [
        ".stado/",
        "/weles/",
        "/Users/Shared/stado",
        "/Users/Shared/jeden",
    ]
    .iter()
    .any(|root| path.contains(root))
}

/// A job's run count is work it did, not a loop it is in.
///
/// A one-shot with `KeepAlive` is invisible to every other check here: it
/// reads `active`, it exits, launchd restarts it, forever. Nothing reported
/// the count, so on charless-mac-mini
/// `com.wisent.compute.service.com.wisent.claude-reauth-once` — a job whose
/// own name says `once` — had run 45,418 times and exited 1 every single time,
/// into a log nobody read; `stado-resolver` was at 50,863 and `brama-funnel`
/// at 50,436.
///
/// Two findings, deliberately separate. A job looping is one defect; a job
/// whose last exit is non-zero is another, and a host can have either without
/// the other.
fn restart_loops(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.declaring_paths.is_empty() {
            continue;
        }
        if let Some(runs) = unit.runs {
            *measured += 1;
            if runs >= RESTART_LOOP_RUNS {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: format!("a managed job starts fewer than {RESTART_LOOP_RUNS} times"),
                    observed: format!(
                        "launchd has started it {runs} times{}; a one-shot under KeepAlive restarts forever",
                        unit.last_exit
                            .map(|code| format!(", last exit {code}"))
                            .unwrap_or_default()
                    ),
                    command: format!(
                        "stado service label-print {} --host {} then stado host unit-log {} {}",
                        unit.label, target.name, target.name, unit.label
                    ),
                });
                continue;
            }
        }
        // A non-zero last exit on a job the fleet installed, reported whether
        // or not it is also looping: `78`, `128` and `255` all read as
        // "loaded" to every other command.
        let exit = unit.last_exit.or_else(|| unit.status.parse().ok());
        if let Some(code) = exit {
            if code != 0 {
                out.push(Finding {
                    check: RESTART_CHECK,
                    subject: format!("{}:{}", target.name, unit.label),
                    declared: "a managed job's last run succeeded".to_string(),
                    observed: format!(
                        "last exit {code}{}",
                        unit.runs
                            .map(|runs| format!(" after {runs} run(s)"))
                            .unwrap_or_default()
                    ),
                    command: format!("stado host unit-log {} {}", target.name, unit.label),
                });
            }
        }
    }
}

/// The `stado` a shell on the host resolves is the one the channel delivered.
///
/// `~/.cargo/bin/stado` at 0.7.34 shadowed a delivered 0.13.40 on this
/// workstation for a week. 0.7.34 has no `--undeclared`, no `bootout` and no
/// `reap`, so every answer it gave was "this host is clean" — not because the
/// host was, but because that binary could not look. A stale product binary in
/// front of a fresh one is worse than no binary: it answers.
///
/// Compared by resolved real path, so a symlink to the delivered file is not a
/// finding.
fn path_binary(
    target: &ComputeTarget,
    posture: Option<&service::PathBinary>,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
    measured: &mut usize,
) {
    let Some(posture) = posture else {
        notes.push(format!(
            "{}: which stado this host carries could not be read",
            target.name
        ));
        return;
    };
    // Unmeasured is not clean. The first version of this check reported
    // agreement whenever `command -v stado` answered nothing, which on
    // charless-mac-mini it always does: the channel's shell is not a login
    // shell. A check that passes because it could not look is the disease.
    if !posture.measurable() {
        notes.push(format!(
            "{}: stado copies UNMEASURED — delivered {} resolved to {:?}, {} location(s) answered",
            target.name,
            posture.delivered,
            posture.delivered_real,
            posture.candidates.len()
        ));
        return;
    }
    *measured += posture.candidates.len();
    let shadows = posture.shadows();
    if shadows.is_empty() {
        notes.push(format!(
            "{}: {} stado location(s) all resolve to the delivered {} ({})",
            target.name,
            posture.candidates.len(),
            posture.delivered_real,
            if posture.delivered_version.is_empty() {
                "version unread"
            } else {
                &posture.delivered_version
            }
        ));
        return;
    }
    for copy in shadows {
        out.push(Finding {
            check: SHADOW_CHECK,
            subject: format!("{}:{}", target.name, copy.path),
            declared: format!(
                "every stado on this host is the delivered {} ({})",
                posture.delivered_real,
                if posture.delivered_version.is_empty() {
                    "version unread"
                } else {
                    &posture.delivered_version
                }
            ),
            observed: format!(
                "{} is {} and resolves to {}, which is not the delivered binary",
                copy.path,
                if copy.version.is_empty() {
                    "unrunnable".to_string()
                } else {
                    copy.version.clone()
                },
                copy.real
            ),
            command: format!(
                "ln -sf {} {} on {} — a stale stado answers every question as though the host were clean",
                posture.delivered, copy.path, target.name
            ),
        });
    }
}

/// A unit file declares the variables its own program reads.
///
/// A launchd job inherits almost nothing, so a plist that names none of what
/// its program requires is a unit that cannot work — and it fails on its
/// interval, quietly, forever. `com.wisent.host-health-beacon-collect` on
/// lukasz-macbook carried `HOME` and `PATH` while its program required
/// `STADO_HOST_HEALTH_API_URL`; it failed every five minutes from 12 August
/// into a log nobody read, and the fleet's own beacon age never noticed
/// because the other hosts self-publish.
///
/// The population is scripts under the account's own home — a fleet script,
/// never a system binary — and the subtraction is against
/// [`AMBIENT_VARIABLES`] plus whatever the script defaults for itself, so a
/// script that handles its own absence is not a finding.
fn unit_environment(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        if unit.script_reads.is_empty() {
            continue;
        }
        *measured += 1;
        let missing: Vec<&str> = unit
            .script_reads
            .iter()
            .map(String::as_str)
            .filter(|name| !AMBIENT_VARIABLES.contains(name))
            .filter(|name| !unit.script_assigns.iter().any(|set| set == name))
            .filter(|name| !unit.env_keys.iter().any(|given| given == name))
            .collect();
        if missing.is_empty() {
            continue;
        }
        out.push(Finding {
            check: UNIT_ENV_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "the unit file declares every variable its program reads".to_string(),
            observed: format!(
                "the plist hands it [{}] and the program reads [{}] without a default",
                if unit.env_keys.is_empty() {
                    "nothing".to_string()
                } else {
                    unit.env_keys.join(" ")
                },
                missing.join(" ")
            ),
            command: format!(
                "stado service env-set {} <KEY> <value> --host {} for each, or stado service ensure {}",
                unit.label, target.name, unit.label
            ),
        });
    }
}

/// One label, one declaring domain.
///
/// The launchd domains are read by
/// [`crate::deploy::service::loaded_units`], which reports every unit file it
/// found for a label rather than the first — the change that made this
/// detectable at all.
fn duplicate_domains(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
) {
    for unit in loaded {
        if unit.declaring_paths.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: DOMAIN_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "one unit file per label".to_string(),
            observed: format!(
                "{} unit files declare it: {}",
                unit.declaring_paths.len(),
                unit.declaring_paths.join(", ")
            ),
            command: format!(
                "stado host remove-file {} <the domain that should not own it> then stado service ensure {}",
                target.name, unit.label
            ),
        });
    }
}

/// A loaded label runs the program its own unit file declares, and runs the
/// binary that is on disk now.
///
/// Two facts, one read, and this fleet has had both of them wrong on the same
/// host at the same time. Nothing was looking:
///
/// - `com.wisent.compute.service.stado-local-control-plane` declares
///   `stado coordinator`, and launchd was holding a `stado dashboard` from
///   2026-08-26 under it — a command the product DELETED on 2026-08-19,
///   whose refresh loop forced a disk-cleanup pass every two minutes. Each
///   forced pass stamped the janitor's shared interval, so the queue agent's
///   own pass returned `interval_noop` before reaching a single cleaner, and
///   the always-on mac ran with disk maintenance switched off while every
///   report that read the unit file agreed with itself.
/// - A process older than the binary it executes is running code nobody
///   shipped. `service converge` already answers this per service, one
///   service at a time, by hand; on 2026-08-31 the mini had a delivery land
///   at 07:13Z and labels still executing the previous version hours later,
///   and no sweep said so.
///
/// Both come free with the label read the sweep already does, which is the
/// whole reason to ask them here: the cost of the answer is zero and the cost
/// of not having it was a night.
fn process_identity(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let mut program_checked = 0_usize;
    let mut binary_checked = 0_usize;
    for unit in loaded {
        if unit.runs_declared_program() == Some(false) {
            out.push(Finding {
                check: PROGRAM_CHECK,
                subject: format!("{}:{}", target.name, unit.label),
                declared: unit.declared_program(),
                observed: format!("pid {} runs {}", unit.pid, unit.running_program),
                command: format!(
                    "stado service bootout {} --host {} then stado service ensure {}",
                    unit.label, target.name, unit.label
                ),
            });
        }
        if unit.runs_declared_program().is_some() {
            program_checked += 1;
        }
        // Only where the fleet holds the unit file. This check reads two
        // process timestamps and its remedy is `service converge`, so its
        // population is the units this fleet installed -- evidence it has in
        // hand, not a guess from the label's spelling. Now that the scan
        // enumerates every loaded label, an OS daemon whose binary a system
        // update replaced after boot would otherwise be reported here as a
        // fleet finding, with a remedy that cannot touch it.
        if !unit.declaring_paths.is_empty() && unit.runs_current_binary() == Some(false) {
            out.push(Finding {
                check: BINARY_CHECK,
                subject: format!("{}:{}", target.name, unit.label),
                declared: "the process executes the binary now on disk".to_string(),
                observed: format!(
                    "pid {} started, then {} was replaced {} second(s) later",
                    unit.pid,
                    unit.running_binary().unwrap_or("its binary"),
                    unit.binary_written_after_start().unwrap_or_default()
                ),
                command: format!(
                    "stado service converge {} --host {}",
                    unit.label, target.name
                ),
            });
        }
        if !unit.declaring_paths.is_empty() && unit.runs_current_binary().is_some() {
            binary_checked += 1;
        }
    }
    // A label launchd holds no pid for answers neither question, and saying so
    // is the difference between "every process is right" and "no process was
    // read".
    //
    // The classification counts are here for the same reason. The scan now
    // reads every loaded label rather than only the fleet-prefixed ones, and a
    // count per class is what makes the widening auditable: an operator can see
    // that rows outside the prefix were looked at, and how many, instead of
    // trusting that a filter upstream chose correctly.
    let undeclared = loaded
        .iter()
        .filter(|unit| unit.classification() == "undeclared")
        .count();
    let outside = loaded
        .iter()
        .filter(|unit| unit.classification() == "outside-fleet-prefix")
        .count();
    notes.push(format!(
        "{}: {} loaded label(s) — {} declared, {} undeclared, {} outside the fleet prefix; \
         {} process(es) compared against their declaration, {} against the installed binary",
        target.name,
        loaded.len(),
        loaded.iter().filter(|unit| unit.declared).count(),
        undeclared,
        outside,
        program_checked,
        binary_checked
    ));
}

/// One process per declared service port, and one health verdict that agrees
/// with the routes behind it.
///
/// Both come out of the same inventory read: it reports the loopback listeners
/// a host holds and the service directory it is supposed to satisfy.
/// Returns the inventory it read, so a second question about the same host is
/// a second judgement rather than a second round trip.
async fn listener_count(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
) -> Option<Value> {
    let reading = match crate::deploy::host_inventory::inventory_target(
        target,
        registry.service_directory.as_ref(),
        runner,
    )
    .await
    {
        Ok(reading) => reading,
        Err(error) => {
            out.push(Finding {
                check: PORT_CHECK,
                subject: target.name.clone(),
                declared: "the host answers a listener inventory".to_string(),
                observed: format!("inventory failed: {error}"),
                command: format!("stado host inventory {}", target.name),
            });
            return None;
        }
    };
    let listeners = reading
        .get("listeners")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = reading
        .get("listeners_state")
        .and_then(Value::as_str)
        .unwrap_or("");
    // A table nobody could read is not an empty table. Without this the check
    // would report "one listener per port" on a host whose `lsof` failed,
    // which is the exact false pass this module exists to refuse.
    if listeners.is_empty() && state != crate::deploy::host_inventory::LISTENERS_READ {
        out.push(Finding {
            check: PORT_CHECK,
            subject: target.name.clone(),
            declared: "the host reports its listeners".to_string(),
            observed: format!("listener table unread ({state})"),
            command: format!("stado host inventory {}", target.name),
        });
        return Some(reading);
    }
    let mut holders: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for listener in &listeners {
        let Some(port) = listener.get("port").and_then(Value::as_u64) else {
            continue;
        };
        let who = format!(
            "{}:{port} pid {}",
            listener
                .get("address")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            listener.get("pid").and_then(Value::as_u64).unwrap_or(0),
        );
        holders.entry(port).or_default().push(who);
    }
    for (port, who) in holders {
        if who.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: PORT_CHECK,
            subject: format!("{}:{port}", target.name),
            declared: "one process serves one declared port".to_string(),
            observed: format!("{} processes hold it: {}", who.len(), who.join(" | ")),
            command: format!(
                "stado service list --undeclared --host {0} and stado service reap --host {0} --command <substring> (report first, then --apply)",
                target.name
            ),
        });
    }
    Some(reading)
}

/// A service unit's artefact is not older than the program the fleet has
/// installed for it.
///
/// The gap this closes is one level below every other version check in the
/// product. `service converge` compares the DECLARED products under
/// `$HOME/<root>/<name>` against the registry and reports `in-sync`;
/// `loaded-label-runs-declared-program` compares a unit file against its
/// process and passes when they agree. Neither can see a unit whose program
/// is `$HOME/.stado/services/<label>/current/<platform>/<program>`, because
/// that artefact is versioned by content hash and the declaration names the
/// `current` link rather than what it points at.
///
/// On 2026-08-31 that blind spot held the fleet's object API — the store
/// behind `stado://probierz` and the release ingress — on an artefact from
/// before 2026-08-19 while the same host's `.stado/bin/stado` had been
/// 0.13.13 since that morning, `service converge` read `in-sync`, and the
/// plist and the process agreed with each other all day. That artefact
/// predates #158 and #168, which is why a replica whose replication was
/// switched off kept being written to, and it predates #206, which is why a
/// state file an operator reads had a writer nobody could name.
fn service_artefacts(
    target: &ComputeTarget,
    reading: &Value,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let artefacts: Vec<crate::deploy::host_inventory::ServiceArtifact> = reading
        .get("service_artifacts")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut compared = 0_usize;
    for artefact in &artefacts {
        // A version the host read from both files answers the question an
        // mtime only gestures at, so it is judged first and the timestamps
        // are the fallback for a program that cannot be asked.
        if !artefact.artefact_version.is_empty()
            && !artefact.installed_version.is_empty()
            && artefact.artefact_version != artefact.installed_version
        {
            out.push(Finding {
                check: ARTEFACT_CHECK,
                subject: format!("{}:{}", target.name, artefact.label),
                declared: format!("the artefact is {}", artefact.installed_version),
                observed: format!(
                    "current -> {} answers {}",
                    artefact.current_target, artefact.artefact_version
                ),
                command: format!(
                    "stado service release {} --host {} (a service release, then the unit restarts onto it)",
                    artefact.label, target.name
                ),
            });
            compared += 1;
            continue;
        }
        match artefact.at_least_as_new_as_installed() {
            Some(false) => {
                let behind = artefact.seconds_behind_installed().unwrap_or_default();
                out.push(Finding {
                    check: ARTEFACT_CHECK,
                    subject: format!("{}:{}", target.name, artefact.label),
                    declared: format!(
                        "the artefact runs code no older than $HOME/.stado/bin/{}",
                        artefact.program
                    ),
                    observed: format!(
                        "current -> {} is {} day(s) older than the installed {}",
                        artefact.current_target,
                        behind / 86_400,
                        artefact.program
                    ),
                    command: format!(
                        "stado service release {} --host {} (a service release, then the unit restarts onto it)",
                        artefact.label, target.name
                    ),
                });
                compared += 1;
            }
            Some(true) => compared += 1,
            // No mtime for one side: nothing was proven, and saying so is the
            // difference between "every artefact is current" and "no artefact
            // was read".
            None => {}
        }
    }
    notes.push(format!(
        "{}: {} service artefact(s) found, {} compared against their installed program",
        target.name,
        artefacts.len(),
        compared
    ));
}

/// Each janitor cleaner and the first released `stado` that accepts it in a
/// registry policy.
///
/// Derived from the tags, not guessed: `git tag --contains` on the commit that
/// added each name to `crate::targets`'s allowed list answers `stado-v0.12.0`
/// for `queue_workdirs` (#154) and `stado-v0.13.0` for `backup_twins`. The two
/// cleaners already in 0.9.5 — `build_caches`, `chromium_clones`,
/// `huggingface_cache`, `weles_recordings` — need no entry, because no host in
/// this fleet runs anything older.
const CLEANERS_BY_VERSION: &[(&str, &str)] =
    &[("queue_workdirs", "0.12.0"), ("backup_twins", "0.13.0")];

/// Whether `installed` is at least `required`, comparing `X.Y.Z` numerically.
///
/// An unreadable or absent version answers false, so an unknown host is treated
/// as unable to take a new cleaner rather than assumed able: the cost of being
/// wrong that way is a note, and the cost of being wrong the other way is a
/// policy that stops every cleaner the host runs.
fn version_at_least(installed: &str, required: &str) -> bool {
    let parse = |value: &str| -> Option<(u64, u64, u64)> {
        let bare = value.trim().trim_start_matches('v');
        let bare = bare.split('-').next().unwrap_or_default();
        let mut parts = bare.split('.').map(|part| part.parse::<u64>().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    match (parse(installed), parse(required)) {
        (Some(installed), Some(required)) => installed >= required,
        _ => false,
    }
}

/// Free space against the watermark the registry declares, and a finding when
/// a managed host declares no policy at all.
async fn disk_headroom(
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let report = match host_disk::disk_host(&target.name, runner).await {
        Ok(report) => report,
        Err(error) => {
            out.push(Finding {
                check: DISK_CHECK,
                subject: target.name.clone(),
                declared: "the host answers df".to_string(),
                observed: format!("disk read failed: {error}"),
                command: format!("stado host disk {}", target.name),
            });
            return;
        }
    };
    let Some(policy) = target.disk_cleanup.as_ref() else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: "no disk_cleanup policy".to_string(),
            observed: "a managed host with slots and no watermark, so nothing on it is ever \
                       obliged to free space"
                .to_string(),
            command: format!(
                "add targets[{}].disk_cleanup to the registry, then stado registry validate and push",
                target.name
            ),
        });
        return;
    };
    let available_kb = report
        .get("usage")
        .and_then(|usage| usage.get("available_kb"))
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<f64>().ok());
    let Some(available_kb) = available_kb else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!("low watermark {} GiB", policy.low_free_gb),
            observed: "df answered without an available column".to_string(),
            command: format!("stado host disk {} --json", target.name),
        });
        return;
    };
    let free_gib = host_disk::gib_from_blocks(available_kb);
    if free_gib < policy.low_free_gb as f64 {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!(
                "low watermark {} GiB, target {} GiB, mode {}",
                policy.low_free_gb, policy.target_free_gb, policy.mode
            ),
            observed: format!("{free_gib:.1} GiB free, so this host is refusing work"),
            command: format!(
                "stado host reclaim {} --apply --reason <why> and stado host backup-audit {} --reclaim-twins --apply",
                target.name, target.name
            ),
        });
    }
    // A cleaner set that cannot reach what fills the machine is the defect the
    // janitor spent a week not fixing on the always-on mac: it declared
    // huggingface_cache and weles_recordings while cargo build trees and a
    // same-disk replica took the disk down.
    //
    // Gated on the version the host RUNS, and that gate is not a nicety. A
    // registry `disk_cleanup` policy is validated as a whole by whatever binary
    // reads it, so declaring a cleaner an older binary does not recognise stops
    // every cleaner that host already runs. This check said exactly that in its
    // own remedy — "AFTER the host runs a binary that knows it" — and then
    // failed `stado doctor`, which `deploy_stado_rust.sh` runs as a delivery
    // preflight. So the finding blocked the delivery that was the prerequisite
    // for acting on the finding. A check that forbids its own remedy is worse
    // than no check, and weakening it would have been the wrong repair.
    let declared_cleaners: Vec<&str> = policy.cleaners.keys().map(String::as_str).collect();
    let installed = target
        .managed_versions
        .get("stado")
        .map(String::as_str)
        .unwrap_or_default();
    for (expected, since) in CLEANERS_BY_VERSION {
        if declared_cleaners.contains(expected) {
            continue;
        }
        if !version_at_least(installed, since) {
            notes.push(format!(
                "{}: {expected} undeclared and unsupported by installed stado {} (needs {since}) \
                 — deliver first, declare second",
                target.name,
                if installed.is_empty() {
                    "unknown"
                } else {
                    installed
                }
            ));
            continue;
        }
        out.push(Finding {
            check: DISK_CHECK,
            subject: format!("{}:{expected}", target.name),
            declared: format!(
                "cleaners {} on stado {installed}",
                declared_cleaners.join(", ")
            ),
            observed: format!(
                "{expected} is supported by the installed binary and not declared, so the janitor \
                 cannot reclaim what it owns"
            ),
            command: format!(
                "add {expected} to targets[{}].disk_cleanup.cleaners, then stado registry validate and push",
                target.name
            ),
        });
    }
}

/// A replica that can never hold what the primary writes.
///
/// [`Endpoint::cannot_replicate`] is the same predicate the write path and the
/// coordinator's replication both consult; this reports the condition standing
/// rather than waiting for someone to notice 48 GiB of unresolvable objects.
///
/// **Reach: THIS control plane's configuration only.** The pairing that
/// actually produced 48 GiB of unaddressable objects on 2026-08-30 was
/// `charless-mac-mini`'s own — `wc_storage_backend: stado` with
/// `wc_backup_storage_backend: local`, read from that host's config, not from
/// here. This control plane declares `storage.backup: null` and so has nothing
/// to disagree about, which is why this arm reports a note rather than a
/// finding on the fleet it was written for. Extending it means reading each
/// host's resolved config the way `stado host config-show` does, one call per
/// host, and that is the next thing this check needs.
fn replica_addressing(result: &mut Sweep) {
    let primary = Endpoint::configured_primary();
    result.measured += 1;
    // Which of the three states this control plane is in is recorded either
    // way. A check whose quiet result and whose "there was nothing to check"
    // result look identical is the shape this module exists to refuse: on the
    // first live sweep this arm produced no finding and no note, and there was
    // no way to tell from the output whether the pairing was sound or simply
    // never read.
    // What the config FILE declares, beside what the resolver answers. These
    // disagreed on this control plane on 2026-08-31: the file declares
    // `storage.backup.backend = local` with a path, `stado config show`
    // resolves `wc_backup_storage_backend` to empty, `stado doctor`'s backup
    // row passes with "no mandatory S3 replica" — and a `storage ls` in the
    // same worktree printed the mirror refusal naming
    // `local://~/.stado/local-backup`, which requires that key to be set.
    // Two readers, two answers, one declaration: components that believe the
    // replica exists write 48 GiB into it while the diagnostics say there is
    // nothing there and pass.
    let declared_in_file = crate::config_file::load_config_file()
        .ok()
        .and_then(|file| file.get("storage").cloned())
        .and_then(|storage| storage.get("backup").cloned())
        .and_then(|backup| {
            backup
                .get("backend")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|backend| !backend.trim().is_empty());
    let Some(backup) = Endpoint::configured_backup() else {
        match declared_in_file {
            Some(backend) => result.record(Finding {
                check: REPLICA_CHECK,
                subject: format!("{}: storage.backup.backend", primary.describe()),
                declared: format!("the config file declares a {backend} replica"),
                observed: "the resolver answers that no replica is configured, so one half of \
                           this binary writes to a replica the other half says does not exist"
                    .to_string(),
                command: "compare `stado config show` against storage.backup in the config file; \
                          the resolver is the half to fix"
                    .to_string(),
            }),
            None => result
                .notes
                .push(format!("{}: no replica declared", primary.describe())),
        }
        return;
    };
    match primary.cannot_replicate(&backup) {
        Some(refusal) => result.record(Finding {
            check: REPLICA_CHECK,
            subject: format!("{} -> {}", primary.describe(), backup.describe()),
            declared: "storage.backup is a disaster-recovery replica of storage".to_string(),
            observed: refusal,
            command: "stado config set storage.backup.backend \"\" to stop declaring a replica, \
                      or point it at a store of the same kind as the primary"
                .to_string(),
        }),
        None => result.notes.push(format!(
            "{} can replicate to {}",
            primary.describe(),
            backup.describe()
        )),
    }
}

/// Whether a service that reports itself healthy is actually serving.
///
/// Read from the endpoint this control plane is configured to use, which is the
/// one whose answers the fleet depends on. `healthz` answering 200 while its
/// own boundaries are closed is not a healthy service: every authorized route
/// behind it returns 503, which is how a store outage read as a slow link for
/// most of a night.
pub async fn health_disagreement() -> Option<Finding> {
    let url = crate::config::wc_stado_storage_url();
    if url.is_empty() {
        return None;
    }
    let endpoint = format!("{}/healthz", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(u8::BITS as u64))
        .build()
        .ok()?;
    let body: Value = client.get(&endpoint).send().await.ok()?.json().await.ok()?;
    let ok = body.get("ok").and_then(Value::as_bool).unwrap_or_default();
    let degraded = body
        .get("degraded")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let closed: Vec<String> = body
        .get("boundaries")
        .and_then(Value::as_object)
        .map(|boundaries| {
            boundaries
                .iter()
                .filter(|(_, open)| open.as_bool() == Some(false))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    if !ok || !degraded || closed.is_empty() {
        return None;
    }
    Some(Finding {
        check: HEALTH_CHECK,
        subject: endpoint,
        declared: "healthz reports whether the service can do its work".to_string(),
        observed: format!(
            "healthz says ok while {} boundary/boundaries are closed ({}), so every route behind \
             them answers 503",
            closed.len(),
            closed.join(", ")
        ),
        command: "stado service logs com.wisent.always-on.stado-object-api --host <host> names \
                  why the boundary is closed; a credential answer is not fixed by restarting the \
                  process"
            .to_string(),
    })
}

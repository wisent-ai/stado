//! What is this host actually running, and did Stado put it there?
//!
//! On 2026-08-18 `stado release status` printed
//! `brama target=charless-mac-mini desired=0.2.27 observed=unreported` and
//! exited zero. A host that had never once said what it runs was rendered
//! indistinguishable from a healthy one, in the command an operator reaches for
//! to ask exactly that. On the same day two machines were running a skarbiec
//! built on somebody's laptop — 0.2.1 here, 0.2.3 on charless-mac-mini, neither
//! of them in any published release — and the pre-fix binary was stripping the
//! `brama:agent:<id>` tags off a live credential every rotation, which removed a
//! working subscription from the fleet while the credential itself stayed valid.
//! No screen in this fleet could name the program doing it, because no screen
//! knew the program existed.
//!
//! Three separate silences, one shape: the fleet stored **declarations** about
//! software and never an **observation** of it. `managed_versions` says what a
//! host must run. `release_control.desired` says what must be rolled out. A
//! service declaration names a unit and a plist. All three stay true across
//! every release that never reached the box, and none of them is about the bytes
//! on the disk.
//!
//! So this module records the other half, and records it the way
//! [`crate::observations`] records everything else that decays: as a look, taken
//! at a moment, by a named vantage, that goes stale. One row per program:
//! `{ name, path, version, sha256, provenance }`.
//!
//! `provenance` is [`RELEASE`] when those exact bytes are also a staged release
//! artefact under `$HOME/.stado/releases`, and [`UNMANAGED`] otherwise. It is
//! decided by digest and by nothing else, on the host, because a name, a version
//! string and a program's own claim about its provenance all survive one `scp`,
//! and a digest that equals the extracted member of an archive Stado verified
//! against the canonical release manifest does not.
//! [`crate::deploy::host_release`] stages every delivery under its own immutable
//! coordinate and hard-links it into place, so a program that came through the
//! sanctioned channel matches and one that did not, does not — which makes
//! `unmanaged` a finding rather than a gap in what this could measure.
//!
//! **Silence is a failure here, and that is the whole point.** A host with no
//! report, a report older than [`crate::observations::DEFAULT_TTL`], a declared
//! program that is absent, an `unmanaged` program, or a version that disagrees
//! with what the fleet declares are all failures out of [`judge`], each in one
//! sentence that names the host and the exact disagreement.
//!
//! What is deliberately *not* a failure is a program nothing declares. This
//! laptop carries eleven dated backup copies of `stado` in `$HOME/.stado/bin`,
//! none of them running, and failing forever on those is how an operator learns
//! to write `|| true` after the command — at which point the drift this exists
//! to catch stops being noticed again, exactly as
//! `service_converge::report_gate` argues. Every such program is still reported,
//! still counted and still visible in `stado host software`; it just does not
//! decide the gate. Accountability is resolved against the live registry on
//! every read rather than frozen into the record, for the reason
//! [`crate::provenance`] does not store reachability: a declaration added an
//! hour after a report must bring that program into scope, and a stored verdict
//! would still be answering the older question.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deploy::service;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::observations::{self, Freshness, Observation, OBSERVED, UNVERIFIED};
use crate::targets::ComputeTarget;

/// The bytes came out of a release Stado published and verified.
pub const RELEASE: &str = "release";
/// The bytes match no release artefact this host carries. A finding.
pub const UNMANAGED: &str = "unmanaged";
/// The reporter looked and the program would not say. Never rounded to a
/// version, and never rounded to agreement.
pub const UNKNOWN: &str = "unknown";
/// What [`report_fact`] prefixes, named once because [`reported_hosts`] reads it
/// back off the fact.
const REPORT_KIND: &str = "software-report:";

/// The reporter that reads every program on the host and states what it is,
/// embedded in this binary and run as one fixed remote script.
///
/// Kept as a checked-in file rather than a string literal so it is reviewed and
/// read as the shell program it is, exactly as `service_converge::VERSION_PROBE`
/// is. Nothing is installed on the host: the helper channel that used to put
/// scripts there was removed for putting unreviewed ones there, and this travels
/// inside the binary instead.
const REPORT_SOFTWARE: &str = include_str!("../scripts/report-host-software.sh");

/// The canonical fact name for "what is this program on this host".
///
/// One spelling, shared by the writer and by every reader, for the reason
/// [`crate::observations::service_fact`] has one: a fact recorded under one name
/// and looked up under another is a fact with no reader.
pub fn software_fact(name: &str, host: &str) -> String {
    format!("software:{name}@{host}")
}

/// The canonical fact name for "did this host report its software at all".
///
/// A separate fact from the programs it lists, and the one that makes silence
/// legible: per-program rows can only ever say what was there, so without a row
/// for the report itself a host that never answered and a host whose programs
/// were all removed would read identically. It also bounds the report —
/// [`observations::record`] merges and never deletes, so a program gone from the
/// host would otherwise stay on file forever and read as present.
pub fn report_fact(host: &str) -> String {
    format!("{REPORT_KIND}{host}")
}

/// One program on one host, as that host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSoftware {
    /// The program's basename, as an operator would say it.
    pub name: String,
    /// Where it is on the host. Absolute, and the one field here that may
    /// contain a space.
    pub path: String,
    /// What the program says it is, or [`UNKNOWN`].
    pub version: String,
    /// What the bytes are, lowercase hex, or [`UNKNOWN`] when the host has no
    /// way to compute one.
    pub sha256: String,
    /// [`RELEASE`] or [`UNMANAGED`], as the host's own digest comparison
    /// decided. A word from a newer reporter is carried through verbatim rather
    /// than rounded to whichever of these two it resembles.
    pub provenance: String,
}

impl HostSoftware {
    pub fn is_release(&self) -> bool {
        self.provenance == RELEASE
    }

    /// The four fields the fact name does not carry, in the shape
    /// [`Observation`]'s detail keeps them.
    ///
    /// `path` last and unquoted, for the reason the wire format puts it last: it
    /// is the only value that may contain a space, and every reader takes the
    /// rest of the line for it.
    fn detail(&self) -> String {
        format!(
            "version={} sha256={} provenance={} path={}",
            self.version, self.sha256, self.provenance, self.path
        )
    }

    /// The inverse of [`Self::detail`], against a name taken from the fact.
    ///
    /// `None` for anything that is not a whole row. A missing path or a missing
    /// provenance is not a row with a default; completing it would put a
    /// fabricated `unmanaged` in front of an operator about bytes nothing read.
    fn from_detail(name: &str, detail: &str) -> Option<Self> {
        let (head, path) = match detail.split_once("path=") {
            Some((head, path)) => (head, path.trim()),
            None => (detail, ""),
        };
        let mut row = Self {
            name: name.to_string(),
            path: path.to_string(),
            version: UNKNOWN.to_string(),
            sha256: UNKNOWN.to_string(),
            provenance: String::new(),
        };
        for token in head.split_whitespace() {
            if let Some(value) = token.strip_prefix("version=") {
                row.version = value.to_string();
            } else if let Some(value) = token.strip_prefix("sha256=") {
                row.sha256 = value.to_string();
            } else if let Some(value) = token.strip_prefix("provenance=") {
                row.provenance = value.to_string();
            }
        }
        if row.path.is_empty() || row.provenance.is_empty() {
            return None;
        }
        Some(row)
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path,
            "version": self.version,
            "sha256": self.sha256,
            "provenance": self.provenance,
        })
    }

    /// The digest, short enough to read inside a sentence and long enough to
    /// look up. A full 64 characters mid-sentence is a sentence nobody reads.
    fn short_digest(&self) -> &str {
        self.sha256.get(..12).unwrap_or(&self.sha256)
    }
}

/// The newest software report one host has on file, and how old it is.
#[derive(Debug, Clone)]
pub struct Report {
    pub host: String,
    /// One row per program the newest report listed.
    pub rows: Vec<HostSoftware>,
    /// Shell scripts the host carries alongside. Counted rather than rowed: the
    /// retired helper channel left 1393 of them in `$HOME/.stado/bin` on
    /// charless-mac-mini against 28 programs, and a release pipeline produces
    /// none of them — rowing each as `unmanaged` would bury the twenty-eight
    /// answers the report exists to give.
    pub scripts: usize,
    /// How old the fleet's knowledge of this host's software is.
    /// [`Freshness::Never`] is the state that was invisible.
    pub freshness: Freshness,
}

impl Report {
    /// Nothing on file for this host. Kept apart from an empty report: a host
    /// that carries no programs answered, and one that never answered did not.
    pub fn never(host: &str) -> Self {
        Self {
            host: host.to_string(),
            rows: Vec::new(),
            scripts: usize::default(),
            freshness: Freshness::Never,
        }
    }

    /// The state word of the look itself: [`OBSERVED`] when the host answered,
    /// [`UNVERIFIED`] when the look could not happen, `never` when none was ever
    /// taken, or a word from a newer writer carried through.
    pub fn state(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) => row.state.as_str(),
            Freshness::Never => "never",
        }
    }

    /// Why, in the reporter's or the channel's own words. Empty when the host
    /// answered cleanly.
    pub fn refusal(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) if row.state != OBSERVED => {
                row.detail.as_str()
            }
            _ => "",
        }
    }

    /// `just now`, `14m ago`, `stale (3h)` or `never`, in the one spelling every
    /// other freshness column in this tree uses.
    pub fn age(&self) -> String {
        observations::render(&self.freshness)
    }

    pub fn released(&self) -> usize {
        self.rows.iter().filter(|row| row.is_release()).count()
    }

    pub fn unmanaged(&self) -> usize {
        self.rows.iter().filter(|row| !row.is_release()).count()
    }

    pub fn find(&self, name: &str) -> Option<&HostSoftware> {
        self.rows.iter().find(|row| row.name == name)
    }

    /// The counts as one phrase, for a row that has one column to say them in.
    pub fn summary(&self) -> String {
        if matches!(self.freshness, Freshness::Never) {
            return "no report".to_string();
        }
        format!(
            "{} program(s), {} release, {} unmanaged, {} script(s)",
            self.rows.len(),
            self.released(),
            self.unmanaged(),
            self.scripts
        )
    }

    pub fn json(&self) -> Value {
        json!({
            "host": self.host,
            "state": self.state(),
            "observed": self.age(),
            "detail": self.refusal(),
            "reported": self.rows.len(),
            "release": self.released(),
            "unmanaged": self.unmanaged(),
            "scripts": self.scripts,
            "programs": self.rows.iter().map(HostSoftware::json).collect::<Vec<Value>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Reading the host
// ---------------------------------------------------------------------------

/// [`REPORT_SOFTWARE`] with the caller's declarations bound ahead of it.
///
/// The unit files come from the registry and the extra programs from the
/// release-control policy, because both are declarations and declarations live on
/// the control plane. The host is asked to read files and hash bytes; it is never
/// asked which of its files matter, which is how a reporter ends up carrying an
/// opinion the registry never authorized.
fn reporter(units: &[(String, String)], programs: &[String]) -> String {
    let units: Vec<String> = units
        .iter()
        .map(|(kind, path)| format!("{kind}\t{path}"))
        .collect();
    format!(
        "units={}\nprograms={}\n{REPORT_SOFTWARE}",
        shlex_quote(&units.join("\n")),
        shlex_quote(&programs.join("\n"))
    )
}

/// The unit files TARGET declares, as `(kind, path)` pairs for the reporter.
fn declared_units(target: &ComputeTarget) -> Vec<(String, String)> {
    service::declared_services(target)
        .into_iter()
        .filter(|declared| !declared.path.is_empty())
        .map(|declared| (declared.kind, declared.path))
        .collect()
}

/// Ask TARGET what it runs.
///
/// One round trip on the same audited channel `host provenance` reads with, and
/// nothing is installed on the host: the reporter travels with stado, so a
/// failure here is the remote's own words about this read and never a remedy for
/// a delivery channel that no longer exists. Reading what is installed is a
/// status read, so it runs under the channel's ordinary read bound.
pub async fn gather(
    target: &ComputeTarget,
    programs: &[String],
    runner: &Runner,
) -> Result<(Vec<HostSoftware>, usize), DeployError> {
    let script = reporter(&declared_units(target), programs);
    let output = host_channel::run_script_with_timeout(
        target,
        &script,
        host_channel::remote_timeout(),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the software reporter did not complete",
        )));
    }
    Ok(parse(&output.stdout))
}

/// The reporter's stdout, as one row per program plus the script count.
///
/// Line-oriented `key=value` rather than JSON for the reason
/// `service_converge::parse_report` gives: a shell script that has to emit valid
/// JSON emits invalid JSON the first time a path contains a quote. Blank lines
/// and `#` comments are skipped and unknown keys are ignored, so the reporter can
/// add a field without a matching release here.
pub fn parse(stdout: &str) -> (Vec<HostSoftware>, usize) {
    let mut rows: Vec<HostSoftware> = Vec::new();
    let mut scripts = usize::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(body) = line.strip_prefix("software ") {
            // The wire order is the storage order, so one decoder serves both
            // and they cannot disagree about where `path=` begins. Only the name
            // is lifted out, because it is the half the fact name carries.
            let Some((_, rest)) = body.split_once("name=") else {
                continue;
            };
            let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            if name.is_empty() {
                continue;
            }
            if let Some(row) = HostSoftware::from_detail(name, rest) {
                rows.push(row);
            }
        } else if let Some(body) = line.strip_prefix("report ") {
            for token in body.split_whitespace() {
                if let Some(value) = token.strip_prefix("scripts=") {
                    scripts = value.parse().unwrap_or_default();
                }
            }
        }
    }
    (rows, scripts)
}

// ---------------------------------------------------------------------------
// Keeping the newest report
// ---------------------------------------------------------------------------

/// The roster row's detail: what the newest report listed, so a later read can
/// tell the report apart from every row ever written for this host.
fn roster_detail(names: &[&str], scripts: usize) -> String {
    format!(
        "reported={} scripts={scripts} names={}",
        names.len(),
        names.join(",")
    )
}

/// Persist one host's report, replacing whatever was on file for it.
///
/// Written through [`observations::record`], the file that already answers "when
/// did anyone last look" for this fleet, because a second store for the same kind
/// of fact is a second answer to that question. The roster row is what makes
/// replacement expressible in a store that merges and never deletes: a program
/// dropped from the newest report is dropped from the roster, so it stops being
/// part of the report even though its own row is still on file.
///
/// The vantage is the target, not this machine. The look happened on that host —
/// its files, its digests, its programs — and recording an operator's laptop as
/// the vantage would let two operators' runs overwrite each other's evidence
/// about a third machine.
pub fn record(host: &str, rows: &[HostSoftware], scripts: usize) -> io::Result<()> {
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let mut written: Vec<Observation> = rows
        .iter()
        .map(|row| Observation::now(software_fact(&row.name, host), host, OBSERVED, row.detail()))
        .collect();
    written.push(Observation::now(
        report_fact(host),
        host,
        OBSERVED,
        roster_detail(&names, scripts),
    ));
    observations::record(&written)
}

/// Record that the look could not happen, in the channel's own words.
///
/// A failed read is written and not swallowed, because the alternative leaves the
/// previous report on file looking current — the exact shape of the twelve-day
/// outage [`crate::observations`] was built against. The roster keeps the names it
/// had, so the last thing anyone saw is still readable and is now visibly
/// unverified.
pub fn record_refusal(host: &str, detail: &str) -> io::Result<()> {
    let held = load(host);
    let names: Vec<&str> = held.rows.iter().map(|row| row.name.as_str()).collect();
    observations::record(&[Observation::now(
        report_fact(host),
        host,
        UNVERIFIED,
        format!("{} {detail}", roster_detail(&names, held.scripts)),
    )])
}

/// The roster row for one host: its freshness, the program names it listed, and
/// the script count beside them.
fn roster(records: &[Observation], host: &str) -> Option<(Freshness, BTreeSet<String>, usize)> {
    let fact = report_fact(host);
    let freshness = observations::freshness_in(records, &fact, observations::DEFAULT_TTL);
    let row = match &freshness {
        Freshness::Fresh(row) | Freshness::Stale(row) => row.clone(),
        Freshness::Never => return None,
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut scripts = usize::default();
    for token in row.detail.split_whitespace() {
        if let Some(value) = token.strip_prefix("names=") {
            names.extend(
                value
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
        } else if let Some(value) = token.strip_prefix("scripts=") {
            scripts = value.parse().unwrap_or_default();
        }
    }
    Some((freshness, names, scripts))
}

/// The newest report on file for one host.
pub fn load(host: &str) -> Report {
    load_in(&observations::load(), host)
}

/// [`load`] against records already in hand, for a reader asking about every
/// target in one rendering — the same reason [`observations::describe_in`]
/// exists.
pub fn load_in(records: &[Observation], host: &str) -> Report {
    let Some((freshness, names, scripts)) = roster(records, host) else {
        return Report::never(host);
    };
    let rows: Vec<HostSoftware> = names
        .iter()
        .filter_map(|name| {
            let fact = software_fact(name, host);
            records
                .iter()
                .filter(|row| row.fact == fact)
                .max_by(|left, right| left.at.cmp(&right.at))
                .and_then(|row| HostSoftware::from_detail(name, &row.detail))
        })
        .collect();
    Report {
        host: host.to_string(),
        rows,
        scripts,
        freshness,
    }
}

/// Every host that has a software report on file.
pub fn reported_hosts(records: &[Observation]) -> Vec<String> {
    let mut hosts: Vec<String> = records
        .iter()
        .filter_map(|row| row.fact.strip_prefix(REPORT_KIND).map(str::to_string))
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

// ---------------------------------------------------------------------------
// Judging
// ---------------------------------------------------------------------------

/// The release-control product one target is supposed to be running, as a
/// concrete file on that host.
#[derive(Debug, Clone)]
pub struct ProductBinary {
    /// The program's basename, which is how the host reports it.
    pub name: String,
    /// The absolute path the release policy installs it at.
    pub path: String,
    /// `None` when the registry declares no desired release, which is a
    /// different finding from a disagreement.
    pub desired: Option<String>,
}

/// What one target's software report says about the declarations it is supposed
/// to satisfy.
#[derive(Debug, Clone, Default)]
pub struct Finding {
    /// True when this target is in a state an operator has to act on.
    pub failed: bool,
    /// One sentence per disagreement, each naming the host and the exact
    /// disagreement. Empty when there is nothing to say.
    pub sentences: Vec<String>,
}

impl Finding {
    /// The one word a screen sorts and colours on. `ok` is only ever reached by a
    /// fresh report in which every declared program is accounted for.
    pub fn word(&self) -> &'static str {
        if self.failed {
            "failed"
        } else {
            "ok"
        }
    }

    /// The verdict, folded into the report it is about.
    ///
    /// One object rather than a nested one, so a consumer reads `verdict` and
    /// `findings` beside the counts they were computed from. `verdict` and not
    /// `state`: the report already carries a `state` — whether the look
    /// happened — and two different questions under one key is how a screen
    /// comes to colour "nobody looked" as "everything is wrong", or worse, the
    /// other way round.
    pub fn merge_into(&self, report: &mut Value) {
        let Some(object) = report.as_object_mut() else {
            return;
        };
        object.insert("verdict".to_string(), json!(self.word()));
        object.insert("failed".to_string(), json!(self.failed));
        object.insert("findings".to_string(), json!(self.sentences));
    }

    pub fn json(&self) -> Value {
        let mut value = json!({});
        self.merge_into(&mut value);
        value
    }

    fn fail(&mut self, sentence: String) {
        self.failed = true;
        self.sentences.push(sentence);
    }
}

/// Everything wrong with one program, in one sentence, or nothing.
///
/// One sentence per program rather than one per fault: an operator reading a gate
/// wants the row and everything the fleet has against it, and splitting
/// "unmanaged" from "wrong version" into two lines about one file makes the
/// output twice as long without adding a fact.
fn disagreement(host: &str, row: &HostSoftware, declared: Option<&str>) -> Option<String> {
    let mut faults: Vec<String> = Vec::new();
    if !row.is_release() {
        faults.push(format!(
            "its digest {} matches no release artefact Stado published, so it is {}",
            row.short_digest(),
            row.provenance
        ));
    }
    match declared {
        Some(want) if row.version == UNKNOWN => faults.push(format!(
            "it reports no version at all, so the declared {want} cannot be confirmed"
        )),
        Some(want) if want != row.version => faults.push(format!("the fleet declares {want}")),
        _ => {}
    }
    if faults.is_empty() {
        return None;
    }
    Some(format!(
        "{host} runs {} {} at {}: {}",
        row.name,
        row.version,
        row.path,
        faults.join(", and ")
    ))
}

/// Does this host's newest report account for what the fleet declares it runs?
///
/// `declared` is the host's `managed_versions`: name to exact version, the same
/// primitive `service converge` and `host release` judge against. `product` is
/// the release-control binary rolled out to this target, which is declared
/// somewhere else entirely and lives under the product's own install root, so it
/// appears in none of the `managed_versions` entries.
///
/// Every failure here is a state an operator has to act on, and every one of them
/// was previously either invisible or printed beside a zero exit.
pub fn judge(
    report: &Report,
    declared: &BTreeMap<String, String>,
    product: Option<&ProductBinary>,
) -> Finding {
    let mut finding = Finding::default();
    let host = report.host.as_str();

    match &report.freshness {
        Freshness::Never => {
            finding.fail(format!(
                "{host} has never reported what software it runs, so every version claimed for it \
                 is a declaration nothing on the host confirms: run `stado host software {host}`"
            ));
            return finding;
        }
        Freshness::Stale(_) => finding.fail(format!(
            "{host} last reported its software {}, past the window an observation speaks for, so \
             nothing here describes the present: run `stado host software {host}`",
            report.age()
        )),
        Freshness::Fresh(_) => {}
    }
    if report.state() != OBSERVED {
        finding.fail(format!(
            "{host} could not report its software ({}): {}",
            report.state(),
            report.refusal()
        ));
        return finding;
    }

    // The registry's per-binary statement of what this host must run, checked
    // against the bytes. `service converge` makes this comparison on versions
    // alone; the digest half is what tells a delivered build apart from one
    // somebody carried over by hand at the same version number.
    for (name, want) in declared {
        match report.find(name) {
            None => finding.fail(format!(
                "{host} declares {name} {want} and its software report names no {name} program at \
                 all, so the declaration is unconfirmed on the host that carries it"
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, Some(want)) {
                    finding.fail(sentence);
                }
            }
        }
    }

    // The rollout's own binary. Matched on its declared path first: the product
    // install root is where the release puts it, and a same-named program
    // elsewhere on the host is a different file.
    if let Some(product) = product {
        let found = report
            .rows
            .iter()
            .find(|row| row.path == product.path)
            .or_else(|| report.find(&product.name));
        match found {
            None => finding.fail(format!(
                "{host} reports no {} program at {}, so the desired {} is confirmed nowhere on the \
                 host it rolls out to",
                product.name,
                product.path,
                product.desired.as_deref().unwrap_or("release")
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, product.desired.as_deref()) {
                    finding.fail(sentence);
                }
            }
        }
    }

    finding
}

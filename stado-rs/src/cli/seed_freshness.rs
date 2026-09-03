//! `stado host authenticator-seed-freshness` — is each login row's stored
//! authenticator seed still the one its account has enrolled?
//!
//! # Why the name, and why Stado owns it
//!
//! The answer is a join of two things neither of which can settle it alone, so
//! the command is named after what it reports — authenticator seed freshness —
//! rather than after Weles, which merely happens to have produced the log
//! lines.
//!
//! The vault knows whether a seed EXISTS. It cannot know whether that seed
//! still MATCHES an enrolment, because matching is only observable where a
//! computed code is submitted to a provider and accepted or refused. So
//! `skarbiec totp-seed-state` answers the vault's half — `present`,
//! `declared_empty`, `field_absent` — and deliberately stops there.
//!
//! The other half is written by the sign-in loop. Brama's journal
//! (`$HOME/.brama/journal.jsonl`) appends one `subscription_sign_in` record
//! per attempt carrying the exact `login_item`, the verdict, the instant, and
//! the trajectory's own tail — which is where the Google SSO driver's
//! `[google_sso] …` markers and Google's own sentences land. That is the run
//! history: per-account, timestamped, and the evidence nobody read for six
//! days while a stale seed was resubmitted every thirty minutes.
//!
//! Since one half lives in a vault and the other in a service's journal, the
//! join is fleet-level, and Stado is the only surface that reaches both. It
//! sits beside `weles-activity` and `weles-run-diagnostics`, which is where an
//! operator already looks when asking what the sign-in loop did.
//!
//! # Why it reads the host's own files rather than the Weles API
//!
//! `weles-activity` already reads the run store off the host filesystem and
//! only PROBES the worker API. That matters here: on 2026-09-02 the admission
//! unit on charless-mac-mini was crash-looping on
//! `ERR_MODULE_NOT_FOUND: Cannot find module …/dist/worker/dispatch.js`, so
//! every `weles-run-diagnostics` call failed — which is exactly the state in
//! which somebody asks this question. A diagnostic that depends on the thing
//! that is broken answers nothing.
//!
//! # What never crosses the channel
//!
//! Not the seed, not a password, not a one-time code, and not the journal's
//! raw `detail` — that field carries up to 1800 characters of trajectory tail
//! and rendered page text. The host-side reader matches a fixed marker
//! vocabulary and returns marker NAMES and counts only. Nothing here computes
//! a code; `skarbiec totp` is deliberately not the call this makes.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// The exact operator path that stores a new seed, as
/// `skarbiec/scripts/store-login-totp-seed.sh` documents itself: the seed
/// arrives on standard input, never in an argument, because an authenticator
/// secret on a command line is a secret in every process table on the host.
const SEED_REPAIR_COMMAND: &str = "printf '%s' '<seed from the authenticator app>' \
     | ACCOUNT=<login-item> skarbiec/scripts/store-login-totp-seed.sh";

/// What the vault said about one row, as `skarbiec totp-seed-state` spells it.
pub const SEED_PRESENT: &str = "present";
pub const SEED_DECLARED_EMPTY: &str = "declared_empty";
pub const SEED_FIELD_ABSENT: &str = "field_absent";
pub const SEED_UNREADABLE: &str = "unreadable";
/// The host's Skarbiec build has no `totp-seed-state`, so the vault's half is
/// unavailable there. Reported as its own condition rather than guessed at:
/// the run history's half is still evidence, and a diagnostic that answers
/// nothing because one source is missing is the failure this command exists to
/// correct.
pub const SEED_READ_UNSUPPORTED: &str = "vault_read_unsupported";

/// One sign-in attempt, reduced to the facts a freshness verdict turns on.
///
/// `code_submitted` is the load-bearing one. An attempt that never reached the
/// authenticator step proves nothing about the seed, and counting it as a
/// rejection is how "the provider is down" would get misreported as "the seed
/// is stale" — a different condition with a different repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub at: String,
    pub at_ms: i64,
    pub result: String,
    /// A code computed from the stored seed was typed into the challenge.
    pub code_submitted: bool,
    /// The provider refused that code, in its own words or after retries.
    pub code_rejected: bool,
    /// Google answered "Too many failed attempts" — the authenticator method
    /// is locked, which is a consequence of resubmitting a stale seed and
    /// blocks the operator's own repair until it clears.
    pub locked_out: bool,
    /// The authenticator step was never usable, so this attempt is silent
    /// about the seed.
    pub authenticator_unreached: bool,
    /// The markers the host matched, for an operator who wants the trail.
    pub markers: Vec<String>,
}

/// The verdict for one login row. Six outcomes, because collapsing any two of
/// them would name the wrong repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Seed present, and the newest attempt that actually submitted a code was
    /// not refused. The seed matched an enrolment as recently as this instant.
    LastKnownGood { at: String },
    /// Seed present, and every attempt that submitted a code since this
    /// instant was refused. The stored seed no longer matches the enrolment.
    RejectedSince {
        since: String,
        attempts: usize,
        locked_out: bool,
    },
    /// Seed present, but nothing has ever submitted a code from it, so its
    /// freshness is untested rather than good.
    PresentUntested,
    /// Seed present and sign-ins are failing, but never at the authenticator
    /// step. Not a seed condition: do not re-enrol on this verdict.
    PresentFailingElsewhere { attempts: usize },
    /// The row's kind declares `totp_secret` and it carries nothing usable.
    FieldEmpty,
    /// The row's kind has no `totp_secret` field at all.
    FieldAbsent,
    /// The vault could not open the row; nothing about the seed is known.
    VaultRowUnreadable,
    /// This host's Skarbiec cannot be asked for seed state at all.
    VaultReadUnsupported,
}

impl Verdict {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LastKnownGood { .. } => "seed_last_known_good",
            Self::RejectedSince { .. } => "seed_rejected_since",
            Self::PresentUntested => "seed_present_untested",
            Self::PresentFailingElsewhere { .. } => "seed_present_failing_elsewhere",
            Self::FieldEmpty => "seed_field_empty",
            Self::FieldAbsent => "seed_field_absent",
            Self::VaultRowUnreadable => "vault_row_unreadable",
            Self::VaultReadUnsupported => "vault_read_unsupported",
        }
    }

    /// Whether an operator has to do something. `PresentUntested` is not a
    /// fault; `PresentFailingElsewhere` is a fault whose repair is elsewhere.
    pub fn needs_reenrolment(&self) -> bool {
        matches!(self, Self::RejectedSince { .. } | Self::FieldEmpty)
    }

    /// The repair, naming the exact command where one exists.
    pub fn repair(&self, login_item: &str) -> String {
        match self {
            Self::RejectedSince { locked_out, .. } => {
                let lockout = if *locked_out {
                    " Google has locked the authenticator method on this account, so re-enrolment \
                     has to wait for that lockout to clear."
                } else {
                    ""
                };
                format!(
                    "re-enrol Google Authenticator on this account, then store the new seed: {}.{}",
                    SEED_REPAIR_COMMAND.replace("<login-item>", login_item),
                    lockout
                )
            }
            Self::FieldEmpty => format!(
                "this row declares totp_secret and carries nothing; enrol Google Authenticator \
                 and store the seed: {}",
                SEED_REPAIR_COMMAND.replace("<login-item>", login_item)
            ),
            Self::FieldAbsent => String::from(
                "this row's kind declares no totp_secret field, so no sign-in of it can answer an \
                 authenticator prompt; store the account as a `login` item before storing a seed",
            ),
            Self::PresentFailingElsewhere { .. } => String::from(
                "sign-ins are failing before the authenticator step, so the seed is not the \
                 condition; read the attempt markers and repair that cause instead",
            ),
            Self::VaultRowUnreadable => String::from(
                "the vault could not open this row; repair vault access before judging its seed",
            ),
            Self::VaultReadUnsupported => String::from(
                "this host's Skarbiec has no `totp-seed-state`, so only the sign-in history \
                 half of the verdict is available; release a Skarbiec carrying it to complete it",
            ),
            Self::LastKnownGood { .. } | Self::PresentUntested => String::new(),
        }
    }
}

/// Decide one row's verdict from the vault's half and the run history's half.
///
/// `attempts` may arrive in any order; freshness is about the newest, so this
/// sorts rather than trusting a reader.
pub fn classify(seed_state: &str, attempts: &[Attempt]) -> Verdict {
    match seed_state {
        SEED_FIELD_ABSENT => return Verdict::FieldAbsent,
        SEED_DECLARED_EMPTY => return Verdict::FieldEmpty,
        SEED_UNREADABLE => return Verdict::VaultRowUnreadable,
        SEED_READ_UNSUPPORTED => return Verdict::VaultReadUnsupported,
        _ => {}
    }

    let mut ordered: Vec<&Attempt> = attempts.iter().collect();
    ordered.sort_by_key(|attempt| attempt.at_ms);

    let submitting: Vec<&&Attempt> = ordered
        .iter()
        .filter(|attempt| attempt.code_submitted)
        .collect();
    if submitting.is_empty() {
        // Nothing ever put a code from this seed in front of the provider.
        // Failures that never reached the step are a different condition, and
        // no failures at all is simply untested.
        let unreached = ordered
            .iter()
            .filter(|attempt| attempt.authenticator_unreached || attempt.result != "signed_in")
            .count();
        return if unreached > 0 {
            Verdict::PresentFailingElsewhere {
                attempts: unreached,
            }
        } else {
            Verdict::PresentUntested
        };
    }

    // The newest submission that was NOT refused is the last moment this seed
    // is known to have matched. Everything after it, if every one of those was
    // refused, is the streak that proves the seed no longer matches.
    let last_good = submitting
        .iter()
        .rposition(|attempt| !attempt.code_rejected);
    match last_good {
        Some(index) if index + 1 == submitting.len() => Verdict::LastKnownGood {
            at: submitting[index].at.clone(),
        },
        other => {
            let start = other.map_or(0, |index| index + 1);
            let streak = &submitting[start..];
            Verdict::RejectedSince {
                since: streak
                    .first()
                    .map(|attempt| attempt.at.clone())
                    .unwrap_or_default(),
                attempts: streak.len(),
                locked_out: streak.iter().any(|attempt| attempt.locked_out),
            }
        }
    }
}

/// Parse one host-side evidence document into attempts, keyed by login item.
pub fn attempts_of(evidence: &Value, login_item: &str) -> Vec<Attempt> {
    evidence
        .get("attempts")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("login_item").and_then(Value::as_str) == Some(login_item))
                .map(|row| {
                    let flag = |name: &str| row.get(name).and_then(Value::as_bool).unwrap_or(false);
                    Attempt {
                        at: row
                            .get("at")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        at_ms: row.get("at_ms").and_then(Value::as_i64).unwrap_or(0),
                        result: row
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        code_submitted: flag("code_submitted"),
                        code_rejected: flag("code_rejected"),
                        locked_out: flag("locked_out"),
                        authenticator_unreached: flag("authenticator_unreached"),
                        markers: row
                            .get("markers")
                            .and_then(Value::as_array)
                            .map(|names| {
                                names
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Join the vault sweep and the attempt evidence into one report.
pub fn build_report(target: &str, vault: &Value, evidence: &Value) -> Value {
    let rows = vault
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut findings = Vec::new();
    for row in &rows {
        let item = row
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let seed_state = row
            .get("seed_state")
            .and_then(Value::as_str)
            .unwrap_or(SEED_UNREADABLE);
        let attempts = attempts_of(evidence, &item);
        let verdict = classify(seed_state, &attempts);
        // A row with no seed field and no sign-in history is not evidence of
        // anything an operator has to act on today, and reporting every such
        // row would bury the two that matter.
        if matches!(verdict, Verdict::FieldAbsent) && attempts.is_empty() {
            continue;
        }
        let mut markers: Vec<String> = attempts
            .iter()
            .flat_map(|attempt| attempt.markers.clone())
            .collect();
        markers.sort();
        markers.dedup();
        let repair = verdict.repair(&item);
        let repair = if repair.is_empty() {
            Value::Null
        } else {
            json!(repair)
        };
        findings.push(json!({
            "login_item": item,
            "kind": row.get("kind").cloned().unwrap_or(Value::Null),
            "seed_state": seed_state,
            "verdict": verdict.code(),
            "needs_reenrolment": verdict.needs_reenrolment(),
            "attempts_recorded": attempts.len(),
            "code_submitting_attempts": attempts
                .iter()
                .filter(|attempt| attempt.code_submitted)
                .count(),
            "rejected_since": match &verdict {
                Verdict::RejectedSince { since, .. } => json!(since),
                _ => Value::Null,
            },
            "last_known_good_at": match &verdict {
                Verdict::LastKnownGood { at } => json!(at),
                _ => Value::Null,
            },
            "locked_out": matches!(&verdict, Verdict::RejectedSince { locked_out: true, .. }),
            "markers": markers,
            "repair": repair,
        }));
    }
    findings.sort_by_key(|finding| {
        // Rows needing a repair first: this is read by somebody deciding what
        // to do, not browsing an inventory.
        let urgent = finding
            .get("needs_reenrolment")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        (
            !urgent,
            finding
                .get("login_item")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    });
    json!({
        "schema_version": 1,
        "target": target,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "evidence": {
            "journal": evidence.get("journal").cloned().unwrap_or(Value::Null),
            "reauth_runs_seen": evidence.get("reauth_runs_seen").cloned().unwrap_or(Value::Null),
        },
        "login_rows_read": rows.len(),
        "findings": findings,
    })
}

/// The host-side evidence reader. Runs under the host's own node, exactly as
/// `WELES_ACTIVITY_SOURCE` does, and prints one marked JSON line.
///
/// It classifies on the host on purpose: the journal's `detail` carries page
/// text, so matching happens where that text already is and only marker names
/// travel.
pub const SEED_EVIDENCE_SOURCE: &str = r#"const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const home = os.homedir();
const journalPath = process.env.BRAMA_STATE_DIR
  ? path.join(process.env.BRAMA_STATE_DIR, 'journal.jsonl')
  : path.join(home, '.brama', 'journal.jsonl');

// The fixed vocabulary. Every entry is a marker NAME plus the pattern that
// proves it; the matched text itself is never carried out of this process.
const MARKERS = [
  ['code_submitted', /filled Google Authenticator TOTP code/i],
  ['authenticator_wrong_code_after_retries', /authenticator_wrong_code_after_retries/],
  ['google_said_wrong_code', /Wrong code/i],
  ['google_said_too_many_failed_attempts', /Too many failed attempts/i],
  ['authenticator_code_input_missing', /authenticator_code_input_missing/],
  ['authenticator_option_not_clickable', /authenticator_option_not_clickable/],
  ['authenticator_method_not_reached', /authenticator_method_not_reached/],
  ['google_said_wrong_password', /Wrong password|couldn.t sign you in/i],
  ['weles_unreachable', /refused the sign-in request|no reachable Weles|no_trajectory/i],
  // A Weles runtime that cannot load its own modules fails every reauth
  // forever, which looks exactly like a stale seed from the outside and has a
  // completely different repair: fix the release. Observed on
  // charless-mac-mini on 2026-09-02, release sha256-4316e3aa4cbf, as
  // `ERR_MODULE_NOT_FOUND: Cannot find module .../dist/worker/dispatch.js`.
  ['weles_runtime_broken', /ERR_MODULE_NOT_FOUND|Cannot find module/i],
  // Brama refusing a run before Weles drove anything: the 131 records this
  // fleet's journal actually holds are almost all of this shape. No code was
  // submitted, so these are silent about the seed and must classify as a
  // failure elsewhere.
  ['weles_not_attributed', /answered HTTP 401|not attributed to the account/i],
  ['run_timed_out', /timed_out=true/],
];

const attempts = [];
let journalRecords = 0;
let journalPresent = false;

try {
  const text = fs.readFileSync(journalPath, 'utf8');
  journalPresent = true;
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    let record;
    try {
      record = JSON.parse(line);
    } catch {
      continue;
    }
    if (!record || record.kind !== 'subscription_sign_in') continue;
    journalRecords += 1;
    const loginItem = typeof record.login_item === 'string' ? record.login_item : '';
    if (!loginItem) continue;
    const detail = typeof record.detail === 'string' ? record.detail : '';
    const names = [];
    for (const [name, pattern] of MARKERS) {
      if (pattern.test(detail)) names.push(name);
    }
    const has = (name) => names.includes(name);
    const rejected = has('authenticator_wrong_code_after_retries')
      || has('google_said_wrong_code')
      || has('google_said_too_many_failed_attempts');
    // A refusal at the authenticator step is only readable when a code was
    // actually typed. Google answering "Too many failed attempts" is itself
    // proof that codes were submitted and refused, even on a run that gave up
    // before typing another one.
    const submitted = has('code_submitted') || rejected;
    attempts.push({
      login_item: loginItem,
      provider: typeof record.provider === 'string' ? record.provider : null,
      at: typeof record.at === 'string' ? record.at : null,
      at_ms: Number.isFinite(record.at_ms) ? record.at_ms : Date.parse(record.at || '') || 0,
      result: typeof record.result === 'string' ? record.result : null,
      code_submitted: submitted,
      code_rejected: rejected,
      locked_out: has('google_said_too_many_failed_attempts'),
      authenticator_unreached: !submitted && (
        has('authenticator_code_input_missing')
        || has('authenticator_option_not_clickable')
        || has('authenticator_method_not_reached')
      ),
      markers: names,
    });
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

// Corroboration only: how many reauth runs the recording store holds, so a
// report can say whether the journal is the whole picture. Same roots
// `weles-activity` reads, and no artifact content is opened.
let reauthRunsSeen = 0;
const recordingRoots = [
  path.join(home, '.stado/services/weles-admission/current/runtime/recordings'),
];
try {
  const serviceRoot = path.join(home, '.stado/services/weles-admission');
  for (const entry of fs.readdirSync(serviceRoot, { withFileTypes: true })) {
    if (!entry.isDirectory() || !entry.name.startsWith('sha256-')) continue;
    const releaseRoot = path.join(serviceRoot, entry.name);
    for (const platform of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platform.isDirectory()) continue;
      recordingRoots.push(path.join(releaseRoot, platform.name, 'runtime', 'recordings'));
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
try {
  const legacyRoot = path.join(home, '.local/share/weles-worker');
  for (const release of fs.readdirSync(legacyRoot, { withFileTypes: true })) {
    if (!release.isDirectory()) continue;
    const releaseRoot = path.join(legacyRoot, release.name);
    for (const platform of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platform.isDirectory()) continue;
      recordingRoots.push(path.join(releaseRoot, platform.name, 'recordings'));
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
const countedRuns = new Set();
for (const root of recordingRoots) {
  let entries = [];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    continue;
  }
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name === '_costs' || countedRuns.has(entry.name)) continue;
    let actions = [];
    try {
      actions = fs.readdirSync(path.join(root, entry.name), { withFileTypes: true });
    } catch {
      continue;
    }
    if (actions.some((action) => action.isDirectory() && /_reauth$/.test(action.name))) {
      countedRuns.add(entry.name);
      reauthRunsSeen += 1;
    }
  }
}

process.stdout.write(`STADO-SEED-EVIDENCE ${JSON.stringify({
  journal: {
    path_present: journalPresent,
    sign_in_records: journalRecords,
    attributed_attempts: attempts.length,
  },
  reauth_runs_seen: reauthRunsSeen,
  attempts,
})}\n`);
"#;

/// The marker the reader prefixes to its one JSON line, so a login shell's own
/// greeting cannot be mistaken for the report.
pub const SEED_EVIDENCE_MARKER: &str = "STADO-SEED-EVIDENCE ";

/// Read one marked JSON line out of a host reader's stdout.
pub fn parse_marked_line(stdout: &str, marker: &str, what: &str) -> Result<Value, CmdError> {
    let line = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker))
        .next_back()
        .ok_or_else(|| CmdError::click(format!("the {what} read printed no report line")))?;
    serde_json::from_str(line).map_err(|error| {
        CmdError::click(format!("the {what} report is not readable JSON: {error}"))
    })
}

/// Render the report the way an operator reads it.
pub fn render(report: &Value) -> String {
    let mut out = String::new();
    let target = report
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let journal = report.get("evidence").and_then(|e| e.get("journal"));
    let records = journal
        .and_then(|j| j.get("sign_in_records"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let runs = report
        .get("evidence")
        .and_then(|e| e.get("reauth_runs_seen"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let rows = report
        .get("login_rows_read")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    out.push_str(&format!(
        "{target}: {rows} login row(s) read, {records} recorded sign-in(s), {runs} reauth run(s) in the recording store\n"
    ));
    if let Some(detail) = report.get("vault_half_unavailable").and_then(Value::as_str) {
        out.push_str(&format!(
            "  the vault half is unavailable on this host: {detail}\n  \
             seed presence is unknown; what follows is the recorded sign-in \
             history only\n"
        ));
    }
    let empty = Vec::new();
    let findings = report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if findings.is_empty() {
        out.push_str("  no login row carries or declares an authenticator seed\n");
        return out;
    }
    for finding in findings {
        let item = finding
            .get("login_item")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let verdict = finding
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("?");
        // The counts belong on every row, not only on a rejection: they are how
        // a reader tells "no evidence" apart from "evidence that says nothing
        // about the seed".
        let recorded = finding
            .get("attempts_recorded")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let submitting = finding
            .get("code_submitting_attempts")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        out.push_str(&format!(
            "  {verdict:<32} {item}\n    {recorded} recorded attempt(s), {submitting} of them submitted a code\n"
        ));
        if let Some(since) = finding.get("rejected_since").and_then(Value::as_str) {
            let attempts = finding
                .get("code_submitting_attempts")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            out.push_str(&format!(
                "    every code submitted since {since} was refused ({attempts} attempt(s) submitted a code)\n"
            ));
        }
        if let Some(at) = finding.get("last_known_good_at").and_then(Value::as_str) {
            out.push_str(&format!("    a code from this seed was accepted at {at}\n"));
        }
        let markers = finding
            .get("markers")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if !markers.is_empty() {
            out.push_str(&format!("    markers: {markers}\n"));
        }
        if let Some(repair) = finding.get("repair").and_then(Value::as_str) {
            out.push_str(&format!("    repair: {repair}\n"));
        }
    }
    out
}

/// Ask the host for both halves and print the joined verdict.
///
/// Two host reads, both read-only: one Skarbiec sweep for the vault's half and
/// one node reader for the run history's half. The vault sweep is a single
/// invocation on purpose — asking per row would open the vault once per
/// account.
pub async fn authenticator_seed_freshness(
    target: &str,
    login_item: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut arguments = vec![String::from("totp-seed-state")];
    if let Some(item) = login_item {
        arguments.push(item.to_string());
    }
    let vault = remote_seed_state(&resolved, &runner, &home, &arguments).await;
    // One item asked for comes back as one object; the report always joins on
    // a list, so a single row is wrapped rather than special-cased below.
    let mut vault_unsupported = None;
    let vault = match vault {
        Ok(answer) if answer.get("rows").is_some() => answer,
        Ok(answer) => json!({"rows": [answer]}),
        // A host still running a Skarbiec without this read is reported, not
        // fatal: the sign-in history is the half nobody was reading, and it is
        // still here.
        Err(error) if error.to_string().contains("unknown command") => {
            vault_unsupported = Some(error.to_string());
            Value::Null
        }
        Err(error) => return Err(error),
    };

    let mut node = None;
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        let present = crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("-x {candidate}"),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if present {
            node = Some(candidate);
            break;
        }
    }
    let node = node.ok_or_else(|| {
        CmdError::click(format!(
            "{}: Node.js is unavailable on this host",
            resolved.name
        ))
    })?;
    let output = crate::deploy::host_channel::run_program_with_stdin(
        &resolved,
        &[node, "-"],
        SEED_EVIDENCE_SOURCE,
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the sign-in evidence read did not complete: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let evidence = parse_marked_line(&output.stdout, SEED_EVIDENCE_MARKER, "sign-in evidence")?;

    let vault = match &vault_unsupported {
        None => vault,
        Some(_) => {
            // Every account the recorded history names, so the report still
            // has one row per account it has evidence about.
            let mut items: Vec<String> = evidence
                .get("attempts")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.get("login_item").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            items.sort();
            items.dedup();
            json!({"rows": items
                .into_iter()
                .map(|item| json!({
                    "item": item,
                    "kind": "login",
                    "seed_state": SEED_READ_UNSUPPORTED,
                }))
                .collect::<Vec<Value>>()})
        }
    };
    let mut report = build_report(&resolved.name, &vault, &evidence);
    if let Some(detail) = vault_unsupported {
        report["vault_half_unavailable"] = json!(detail);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report));
    }
    Ok(())
}

/// Run one Skarbiec read on the host and parse its JSON answer.
///
/// The vault and GnuPG paths are resolved by the target itself, and arguments
/// stay separate all the way through the host channel, so nothing an operator
/// typed enters a remote shell command. Modelled on `cli::host`'s own Skarbiec
/// reads; kept local because this diagnostic needs exactly one command and no
/// write path.
async fn remote_seed_state(
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    home: &str,
    arguments: &[String],
) -> Result<Value, CmdError> {
    let environment = crate::deploy::host_channel::run_command(
        resolved,
        "printf '%s\\n%s\\n' \"${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}\" \
         \"${GNUPGHOME:-$HOME/.gnupg}\"",
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: the Skarbiec environment could not be read",
            resolved.name
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: the vault path is empty", resolved.name)))?;
    let gnupg_home = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: GNUPGHOME is empty", resolved.name)))?;
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let tool_path = format!(
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/local/MacGPG2/bin:{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let mut invocation = vec![
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
    ];
    invocation.extend(arguments.iter().map(String::as_str));
    let output = crate::deploy::host_channel::run_program(resolved, &invocation, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec totp-seed-state failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec totp-seed-state returned unreadable JSON: {error}",
            resolved.name
        ))
    })
}

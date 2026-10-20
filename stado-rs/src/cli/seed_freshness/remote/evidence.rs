//! The run history's half: the program the host runs to read its own journal
//! and recording store, the marker it prefixes to its one line, and the reader
//! of that line.

use serde_json::Value;

use crate::cli::CmdError;

/// The host-side evidence reader. Runs under the host's own node, exactly as
/// `WELES_ACTIVITY_SOURCE` does, and prints one marked JSON line.
///
/// It classifies on the host on purpose: the journal's `detail` carries page
/// text, so matching happens where that text already is and only marker names
/// travel.
pub(in crate::cli::seed_freshness) const SEED_EVIDENCE_SOURCE: &str = r#"const fs = require('node:fs');
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
  // completely different repair: fix the release. Its signature is
  // `ERR_MODULE_NOT_FOUND: Cannot find module .../dist/worker/dispatch.js`.
  ['weles_runtime_broken', /ERR_MODULE_NOT_FOUND|Cannot find module/i],
  // Brama refusing a run before Weles drove anything: most records in a
  // fleet's journal are of this shape. No code was
  // submitted, so these are silent about the seed and must classify as a
  // failure elsewhere.
  ['weles_not_attributed', /answered HTTP 401|not attributed to the account/i],
  ['run_timed_out', /timed_out=true/],
];

const attempts = [];
const signInRecords = [];
// Records the journal holds but this reader could not take: lines that are
// not JSON, sign-ins naming no login item, and sign-ins carrying no detail
// text to match markers in. Each is reported, never silently skipped, so a
// count over the journal says how much of it was read.
const unreadableLines = [];
const unattributedRecords = [];
let journalPresent = false;

try {
  const text = fs.readFileSync(journalPath, 'utf8');
  journalPresent = true;
  for (const [index, line] of text.split('\n').entries()) {
    if (!line.trim()) continue;
    let record;
    try {
      record = JSON.parse(line);
    } catch (error) {
      unreadableLines.push({ line_index: index, error: String(error?.message ?? error) });
      continue;
    }
    if (!record || record.kind !== 'subscription_sign_in') continue;
    signInRecords.push(index);
    if (typeof record.login_item !== 'string' || !record.login_item) {
      unattributedRecords.push(index);
      continue;
    }
    const loginItem = record.login_item;
    if (typeof record.detail !== 'string') {
      unreadableLines.push({ line_index: index, error: `the sign-in of ${loginItem} carries no detail text` });
      continue;
    }
    const detail = record.detail;
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
    // The instant: `at_ms` when the record carries one, else `at` parsed. A
    // record dated by neither carries null, which the report refuses rather
    // than sorting it as the oldest attempt.
    const atTextMs = typeof record.at === 'string' ? Date.parse(record.at) : Number.NaN;
    const atMs = Number.isFinite(record.at_ms)
      ? record.at_ms
      : (Number.isFinite(atTextMs) ? atTextMs : null);
    attempts.push({
      login_item: loginItem,
      provider: typeof record.provider === 'string' ? record.provider : null,
      at: typeof record.at === 'string' ? record.at : null,
      at_ms: atMs,
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
const unreadableRecordings = [];
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
// A root that does not exist holds no runs; one that exists but cannot be
// listed is reported with its error, so the count is known to be partial.
const listed = (directory) => {
  try {
    return fs.readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    if (error?.code !== 'ENOENT') {
      unreadableRecordings.push({ path: directory, error: String(error?.message ?? error) });
    }
    return [];
  }
};
const countedRuns = new Set();
for (const root of recordingRoots) {
  for (const entry of listed(root)) {
    if (!entry.isDirectory() || entry.name === '_costs' || countedRuns.has(entry.name)) continue;
    const actions = listed(path.join(root, entry.name));
    if (actions.some((action) => action.isDirectory() && /_reauth$/.test(action.name))) {
      countedRuns.add(entry.name);
    }
  }
}

process.stdout.write(`STADO-SEED-EVIDENCE ${JSON.stringify({
  journal: {
    path_present: journalPresent,
    sign_in_records: signInRecords.length,
    attributed_attempts: attempts.length,
    unattributed_records: unattributedRecords.length,
    unreadable_lines: unreadableLines,
  },
  reauth_runs_seen: countedRuns.size,
  unreadable_recordings: unreadableRecordings,
  attempts,
})}\n`);
"#;

/// The marker the reader prefixes to its one JSON line, so a login shell's own
/// greeting cannot be mistaken for the report.
pub(in crate::cli::seed_freshness) const SEED_EVIDENCE_MARKER: &str = "STADO-SEED-EVIDENCE ";

/// Read one marked JSON line out of a host reader's stdout.
pub(in crate::cli::seed_freshness) fn parse_marked_line(
    stdout: &str,
    marker: &str,
    what: &str,
) -> Result<Value, CmdError> {
    let line = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker))
        .next_back()
        .ok_or_else(|| {
            CmdError::click(format!("the {what} read printed no report line"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    serde_json::from_str(line).map_err(|error| {
        CmdError::click(format!("the {what} report is not readable JSON: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

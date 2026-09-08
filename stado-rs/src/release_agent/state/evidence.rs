//! One release's own log: where the agent writes it, and how a refusal quotes
//! it back.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use super::records::{ProcessRecord, QuarantineRecord};
use crate::release_cause;
use crate::release_control::ReleaseTargetPolicy;

/// One release's own stdout or stderr on its host.
///
/// A brama candidate died inside ninety seconds and the rollout record said
/// only `candidate did not become ready within 90s: pid 46748 is gone`, while
/// the candidate's account of itself sat unread in
/// `<logs_root>/brama-0.2.27.err`. The name is public so an operator-facing
/// command reads the file the agent actually wrote rather than a second guess
/// at this format.
pub fn host_log_path(logs_root: &str, product: &str, version: &str, stream: &str) -> String {
    format!("{logs_root}/{product}-{version}.{stream}")
}

fn release_log_path(
    target: &ReleaseTargetPolicy,
    product: &str,
    version: &str,
    stream: &str,
) -> PathBuf {
    PathBuf::from(host_log_path(&target.logs_root, product, version, stream))
}

pub(crate) fn release_log(
    target: &ReleaseTargetPolicy,
    product: &str,
    version: &str,
    stream: &str,
) -> Result<File, String> {
    std::fs::create_dir_all(&target.logs_root)
        .map_err(|error| format!("cannot create release logs {}: {error}", target.logs_root))?;
    let path = release_log_path(target, product, version, stream);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open release log {}: {error}", path.display()))
}

/// One release log as a failure record uses it.
///
/// A quarantine reason used to say only what the agent observed from outside --
/// "pid is gone", "refused the connection" -- and the product's own account of
/// why it exited sat in a file nobody had opened. Two days of one session went
/// into reading those files by hand, one candidate at a time, so the reason now
/// carries the tail with it. Missing or unreadable is reported, never silently
/// dropped: a reason that mentions no log at all would send the next reader
/// hunting for one.
struct LogEvidence {
    /// `<path>: <tail>` for the reason string, or a bracketed note when there
    /// is nothing to quote.
    rendered: String,
    /// Every byte the product wrote, for the classifier. Empty exactly when
    /// the file was missing, unreadable or empty.
    body: String,
}

/// Quote a bounded window of `text` that keeps both of its ends.
///
/// This used to keep the first `max_chars` characters and drop everything
/// after them, and eight of the twenty live `brama` records were clipped that
/// way. Measured against those records the head-clip did not actually lose a
/// cause: every decisive sentence present sits in the first 57% of its tail,
/// the latest being `brama-0.2.55`'s `no value at ...#value`. So this is not a
/// fix for an observed misclassification, and the fix for that is elsewhere --
/// the cause is now derived from the whole log rather than from this window.
///
/// It is still the wrong end to drop. Nothing holds a decisive line near the
/// head: 57% of a 1200-character budget is already past the midpoint, and the
/// end of a dying process's log is exactly where a panic, an abort message or a
/// final error lands. Keeping both ends costs the same width and cannot lose
/// either. The elision carries the count of what went rather than a bare
/// ellipsis, because a reader who cannot see how much was dropped cannot tell a
/// trimmed line from a whole one.
pub(crate) fn clip_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars / 2;
    let tail_chars = max_chars - head_chars;
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(index, _)| index);
    let tail_start = text
        .char_indices()
        .nth(total - tail_chars)
        .map_or(text.len(), |(index, _)| index);
    format!(
        "{} …{} elided… {}",
        &text[..head_end],
        total - max_chars,
        &text[tail_start..]
    )
}

/// Read one release log once, for both the reason and the classifier.
///
/// Read once rather than twice on purpose: the reason quotes a bounded tail and
/// the classifier wants every byte, and opening the file a second time would
/// let the two disagree about what the product said.
fn log_evidence(path: &Path, lines: usize, max_chars: usize) -> LogEvidence {
    let note = |text: String| LogEvidence {
        rendered: text,
        body: String::new(),
    };
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) => return note(format!("[{} unreadable: {error}]", path.display())),
    };
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        return note(format!("[{} is empty]", path.display()));
    }
    let mut kept: Vec<&str> = trimmed.lines().rev().take(lines).collect();
    kept.reverse();
    let clipped = clip_middle(&kept.join(" | "), max_chars);
    LogEvidence {
        rendered: format!("{}: {clipped}", path.display()),
        body,
    }
}

/// Keep the same process evidence for startup, active and drain failures.
/// The reason carries bounded tails; classification reads the full logs once.
pub(crate) fn quarantine_with_logs(
    target: &ReleaseTargetPolicy,
    product: &str,
    record: &ProcessRecord,
    why: &str,
) -> QuarantineRecord {
    let stderr = log_evidence(
        &release_log_path(target, product, &record.version, "err"),
        20,
        1200,
    );
    let stdout = log_evidence(
        &release_log_path(target, product, &record.version, "out"),
        5,
        400,
    );
    let reason = format!(
        "{why}; stderr {}; stdout {}",
        stderr.rendered, stdout.rendered
    );
    let classified = release_cause::classify(&format!(
        "{why}\n{}\n{}",
        stderr.body.trim_end(),
        stdout.body.trim_end()
    ));
    QuarantineRecord::classified(reason, classified)
}

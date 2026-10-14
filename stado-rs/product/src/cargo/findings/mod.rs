//! What a refused gate found, read from the compiler's own report.
//!
//! A gate that refuses names only its argv and exit status, so the author of
//! the refused commit, and the main check that records it, had nothing to
//! repair from but a rerun. Cargo, rustc and clippy report each diagnostic as
//! a header line (`error[E0425]: cannot find function …`, `warning: …`) at
//! the start of a line, followed by indented lines, one of which is the
//! location arrow `--> <file>:<line>:<column>`, and, for a lint, a note naming
//! it (`#[deny(clippy::needless_borrow)]` or `` `-D clippy::…` implied by …``).
//! Each such block becomes one finding: the location, the lint or error code
//! when one is named, and the header's message. Output that carries no
//! location arrow names no finding; the refusal then carries the gate's own
//! words instead.

/// One diagnostic of a refused gate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    /// `<file>:<line>:<column>` as the compiler printed it.
    pub location: String,
    /// The lint (`clippy::needless_borrow`) or error code (`E0425`) the block
    /// names, when it names one.
    pub lint: Option<String>,
    /// The header line, as printed.
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.lint {
            Some(lint) => write!(formatter, "{} [{lint}] {}", self.location, self.message),
            None => write!(formatter, "{} {}", self.location, self.message),
        }
    }
}

/// The location an arrow line names: `--> src/lib.rs:3:5`.
fn arrow(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("--> ").map(str::trim)
}

/// The lint a block's note or help line names (`= note: …`, `= help: …`):
/// inside `#[…(<lint>)]`, or after `-D `/`-W ` in backticks. A source line
/// quoted in the block (`#[derive(Debug)]`) is not a note and names none.
fn noted_lint(line: &str) -> Option<String> {
    let note = line.trim_start().strip_prefix("= ")?;
    if let Some((_, rest)) = note.split_once("#[") {
        let inner = rest.split_once(']').map(|(inner, _)| inner)?;
        let (_, lint) = inner.split_once('(')?;
        return Some(lint.trim_end_matches(')').to_string());
    }
    for flag in ["`-D ", "`-W "] {
        if let Some((_, rest)) = note.split_once(flag) {
            return rest.split_once('`').map(|(lint, _)| lint.to_string());
        }
    }
    None
}

/// The error code a header names in brackets: `error[E0425]: …`.
fn header_code(header: &str) -> Option<String> {
    let (_, bracketed) = header.split_once('[')?;
    let (code, _) = bracketed.split_once(']')?;
    Some(code.to_string())
}

/// Every finding in a gate's report, in the order printed. A header is a
/// line that starts at the margin and is followed, before the next such
/// line, by a location arrow; the block's first arrow is its location.
pub fn read(report: &str) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut header: Option<&str> = None;
    let mut current: Option<Finding> = None;
    for line in report.lines() {
        let at_margin = !line.is_empty() && !line.starts_with(char::is_whitespace);
        if at_margin {
            findings.extend(current.take());
            header = Some(line.trim());
            continue;
        }
        let Some(header) = header else {
            continue;
        };
        if let Some(location) = arrow(line) {
            if current.is_none() {
                current = Some(Finding {
                    location: location.to_string(),
                    lint: None,
                    message: header.to_string(),
                });
            }
            continue;
        }
        if let Some(finding) = current.as_mut().filter(|finding| finding.lint.is_none()) {
            finding.lint = noted_lint(line);
        }
    }
    findings.extend(current);
    for finding in &mut findings {
        if finding.lint.is_none() {
            finding.lint = header_code(&finding.message);
        }
    }
    findings
}

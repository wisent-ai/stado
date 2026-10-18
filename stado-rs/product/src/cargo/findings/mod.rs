//! What a refused Cargo operation found, read from Cargo's own records.
//!
//! A gate that refuses names only its argv and exit status, so the author of
//! the refused commit, and the main check that records it, had nothing to
//! repair from but a rerun. Cargo states each compiler diagnostic as one JSON
//! record when it is run with [`MESSAGE_FORMAT`]: its level, message, lint or
//! error code, the spans it points at and the text a person reads. Each
//! diagnostic with a primary span becomes one finding; the rendered text is
//! handed on so the report a person reads stays the compiler's own.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// The program whose operations report diagnostics as records.
pub const CARGO: &str = "cargo";

/// Cargo's flag that prints every compiler diagnostic as one JSON record,
/// with its rendered text inside it.
pub const MESSAGE_FORMAT: &str = "--message-format=json";

/// The option the flag sets, which a caller may not set otherwise.
const MESSAGE_FORMAT_OPTION: &str = "--message-format";

/// Where Cargo's own options end and the arguments it hands on begin.
const SEPARATOR: &str = "--";

/// How every record Cargo prints begins: its `reason` tag is serialized
/// first. A line that begins otherwise is output of the program Cargo ran.
const RECORD_START: &[u8] = b"{\"reason\":";

/// The record that carries a compiler diagnostic.
const COMPILER_MESSAGE: &str = "compiler-message";

/// One diagnostic of a refused gate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    /// `<file>:<line>:<column>` of the diagnostic's primary span.
    pub location: String,
    /// The lint (`clippy::needless_return`) or error code (`E0425`) the
    /// diagnostic names, when it names one.
    pub lint: Option<String>,
    /// The diagnostic's level and message, as its header reads.
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

#[derive(Deserialize)]
struct Record {
    reason: String,
    message: Option<Diagnostic>,
}

#[derive(Deserialize)]
struct Diagnostic {
    message: String,
    level: String,
    code: Option<Code>,
    spans: Vec<Span>,
    rendered: Option<String>,
}

#[derive(Deserialize)]
struct Code {
    code: String,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    line_start: u64,
    column_start: u64,
    is_primary: bool,
}

/// What a Cargo operation run with [`MESSAGE_FORMAT`] printed on stdout,
/// taken apart.
#[derive(Debug, Default)]
pub struct Messages {
    /// Every diagnostic with a primary span, in the order printed.
    pub findings: Vec<Finding>,
    /// The diagnostics as the compiler renders them for a person.
    pub rendered: String,
    /// Every line that is no Cargo record: the output of what Cargo ran.
    pub rest: Vec<u8>,
}

impl Messages {
    /// Take `stdout` apart. A line that begins as a Cargo record and does not
    /// read as one is refused quoting it: dropped, a diagnostic would be lost.
    pub fn read(stdout: &[u8]) -> Result<Self> {
        let mut messages = Self::default();
        for line in stdout.split_inclusive(|byte| *byte == b'\n') {
            if !line.starts_with(RECORD_START) {
                messages.rest.extend_from_slice(line);
                continue;
            }
            let record: Record = serde_json::from_slice(line).with_context(|| {
                format!(
                    "Cargo printed a record that does not read: {}",
                    String::from_utf8_lossy(line)
                )
            })?;
            let Some(diagnostic) = record.message.filter(|_| record.reason == COMPILER_MESSAGE)
            else {
                continue;
            };
            if let Some(rendered) = &diagnostic.rendered {
                messages.rendered.push_str(rendered);
            }
            let Some(span) = diagnostic.spans.iter().find(|span| span.is_primary) else {
                continue;
            };
            messages.findings.push(Finding {
                location: format!(
                    "{}:{}:{}",
                    span.file_name, span.line_start, span.column_start
                ),
                lint: diagnostic.code.map(|code| code.code),
                message: format!("{}: {}", diagnostic.level, diagnostic.message),
            });
        }
        Ok(messages)
    }
}

/// `arguments` of a Cargo operation with [`MESSAGE_FORMAT`] among Cargo's
/// own options, before any [`SEPARATOR`]. A caller that sets the message
/// format itself is refused: the records this module reads are the ones the
/// flag asks for.
pub fn with_messages(arguments: &[String]) -> Result<Vec<String>> {
    let own = match arguments.iter().position(|argument| argument == SEPARATOR) {
        Some(at) => at,
        None => arguments.len(),
    };
    if let Some(set) = arguments[..own]
        .iter()
        .find(|argument| argument.starts_with(MESSAGE_FORMAT_OPTION))
    {
        bail!(
            "{set}: Stado reads Cargo's diagnostics as the records {MESSAGE_FORMAT} prints and \
             sets the message format itself"
        );
    }
    let mut with = arguments[..own].to_vec();
    with.push(MESSAGE_FORMAT.to_string());
    with.extend_from_slice(&arguments[own..]);
    Ok(with)
}

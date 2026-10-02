//! Credential recovery from structured agent transcripts.
//!
//! Tool results can contain environment dumps, process arguments and file
//! reads. This module inventories recoverable credential material already
//! present in those records.
//!
//! Inventory reports names, counts, dates and locations, not secret values.
//! Recovery callers request an exact name through [`value_for`] and write
//! the result into the selected credential store without passing a shell.
//!
//! # Reading the stores, not scanning them
//!
//! These files have a schema, and using it is the difference between an
//! inventory and a pile of guesses. A flat text scan cannot tell a live
//! environment dump from a transcript of somebody reading a source file, so it
//! reports every `KEY`-ish identifier in the repository as a recoverable
//! credential.
//!
//! Both stores record which tool produced each payload:
//!
//! - `~/.omp/agent/sessions/**/*.jsonl` — newline-delimited events. A tool
//!   result is `{"type":"message","message":{"role":"toolResult",
//!   "toolName":…,"content":[{"type":"text","text":…}]}}`.
//! - `~/.claude/projects/**/*.jsonl` — the result carries a `toolUseResult`
//!   field, and the tool's NAME lives in the earlier assistant event's
//!   `tool_use` block, matched by id. So the file is read in order and the
//!   id-to-name map is carried forward.
//! - `*.bash.log` / `*.eval.log` — genuinely raw captured output, no envelope.
//!
//! [`RUNTIME_TOOLS`] then separates payloads that observed the live machine
//! (a shell, an evaluator) from payloads that merely quoted a file. Only the
//! former can contain a credential that was actually in use.
//!
//! # Components
//!
//! The parts are the seams the account above already has: `model` holds the
//! two reported types, `detect` the name and value shapes that decide what
//! counts as a credential, `sources` the two things that read the stores —
//! the file walk and the per-schema payload extraction — and `queries` the
//! three entry points a caller has: the inventory, the one-name value and
//! the unlock-phrase candidates. The vocabulary all of them share, the raw
//! roots and the runtime tool names, stays here. Every name a caller
//! outside this module uses is re-exported here, so
//! `crate::transcripts::<item>` resolves exactly as before.

mod detect;
mod model;
mod queries;
mod sources;

pub use model::{Finding, Origin};
pub use queries::inventory::scan;
pub use queries::recovery::{unlock_candidates, value_for};
pub use sources::files::transcript_files;

/// Roots that hold unmasked transcripts. The transcript lake under
/// `~/.transcript-lake` is deliberately absent: its ingest masks high-entropy
/// fields, so an armored key or a bearer arrives there already destroyed. These
/// are the raw per-session stores that do not.
const TRANSCRIPT_ROOTS: &[&str] = &[
    "$HOME/.omp/agent/sessions",
    "$HOME/.claude/projects",
    // The mirror, and the largest store of all — tens of thousands of session
    // files. Omitting it made every "not in any transcript" verdict cover a
    // fraction of the evidence.
    "$HOME/.claude/transcripts-repo",
    // Kimi keeps sessions under its own tree with a wire format of its own.
    "$HOME/.kimi-code/sessions",
    "$HOME/.codex",
    "$HOME/.factory",
    "$HOME/.oko",
];

/// Tools whose output describes the live machine rather than a file's contents.
/// A shell or an evaluator prints environments, process tables and command
/// output; a reader or a searcher prints source. Only the first kind can leak a
/// credential that was genuinely in use, and the distinction is what keeps this
/// an inventory instead of a list of variable names from the repository.
const RUNTIME_TOOLS: &[&str] = &["bash", "eval", "hub", "debug", "BashOutput", "Bash"];

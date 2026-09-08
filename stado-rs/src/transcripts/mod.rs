//! Secret material left behind in agent transcripts.
//!
//! Agent runtimes persist every tool call and result. Those results include
//! process listings, environment dumps and file reads, so credentials the fleet
//! never meant to write down are sitting in plain text on disk, dated, in files
//! nobody prunes. During the vault key-loss incident this turned out to be the
//! only surviving copy of several live values.
//!
//! Two consequences, and this module exists for both:
//!
//! - **Recovery.** A vault whose key material is gone can be rebuilt from what
//!   the transcripts already contain.
//! - **Exposure.** The same scan is the inventory of what leaked, which is the
//!   thing to shrink once recovery is done.
//!
//! It reports names, counts, dates and locations. It NEVER returns a secret
//! value: the whole defect being measured is values reaching places that only
//! needed names, and a tool that printed them to a terminal, a log or an agent
//! transcript would be one more of those places. Values move only through
//! [`value_for`], which the caller must ask for by exact name and which writes
//! into the selected credential store without passing a shell.
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

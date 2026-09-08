//! `stado host exec TARGET -- CMD…` — run one APPROVED read-only command
//! on a registry host through the shared ssh channel.
//!
//! NO Python original: item six of `stado.wisent.com/docs/missing-commands`, whose
//! wording is the whole design — "with an allowlist, not free shell".
//!
//! This is not a remote shell and must never become one. `stado host exec`
//! exists so an operator diagnosing a wedged box does not have to keep a
//! private ssh alias outside the registry-authorized channel; it does not
//! exist to run arbitrary code as the login user of every machine in the
//! fleet.
//!
//! Three independent barriers stand between the operator's words and the
//! host, in this order:
//!
//! 1. **Character rejection.** Every word the operator typed must consist
//!    only of characters no shell treats specially ([`is_shell_safe`]).
//!    Anything carrying `;`, `|`, `&`, `$`, backtick, quote, newline,
//!    redirection or glob is refused by name before anything else happens.
//! 2. **Exact allowlist match.** The words, joined, must equal one entry of
//!    [`APPROVED_COMMANDS`] exactly. There is no prefix match, no
//!    pass-through of extra arguments, and no operator-supplied path — a
//!    command that took a path would be a command that could read
//!    `~/.ssh/id_ed25519`.
//! 3. **Fixed argv.** What actually runs is the matched entry's own
//!    `argv`, a compile-time constant of absolute paths. The operator's
//!    words select an entry; they never become part of the command line.
//!    [`crate::deploy::host_channel::ssh_program_argv`] then shell-quotes
//!    each fixed word for the remote login shell.
//!
//! Barrier 3 alone makes injection impossible, which is precisely why
//! barriers 1 and 2 are worth having: they mean the guarantee does not
//! depend on the table being perfectly curated, and they give the operator
//! a real error instead of a silent mismatch.
//!
//! Almost every entry is read-only. The exceptions are the provider sign-in
//! repairs and the fixed Probierz run-root preparation at the end of the
//! table. Those exist because no read can substitute for the bounded repair
//! or preparation; each states in its own [`ApprovedCommand::why`] exactly
//! what it changes. Every entry, read or mutation, still takes no
//! operator-supplied argument and carries its own justification.
//!
//! An entry whose program the managed account owns rather than the system —
//! anything under `~` — is described once more in
//! [`channel::ACCOUNT_PROGRAMS`], which supplies the fixed environment and
//! the time budget that program needs. Those words are compile-time constants
//! of this module too, so barrier three is unchanged: `$HOME` expands on the
//! far side and nothing the operator typed reaches the host except the choice
//! of entry.
//!
//! An entry whose fixed path ARGUMENTS name something inside that account's
//! home — rather than its program — is listed once more in
//! [`allowlist::arguments::HOME_ROOTED_READS`], which runs it from that home.
//! Those paths are written relative for a reason barrier one imposes: `~` is
//! a character a shell acts on, so an argument spelled `~/…` would be refused
//! as the operator's own word and the entry could never be selected at all.

mod allowlist;
mod channel;
mod refusal;
mod run;

pub(crate) use allowlist::is_retained_log_read;
pub use allowlist::{
    allowlist, approve, cargo_candidates, is_shell_safe, program_candidates, ApprovedCommand,
    ADB_PROGRAM, APPIUM_PROGRAM, APPROVED_COMMANDS, GIT_PROGRAM, NODE_PROGRAM, TMUX_PROGRAM,
};
pub use refusal::ExecRefusal;
pub use run::{exec_host, OK_STATUS};

const RESOLVED_EXECUTABLE_MARKER: &str = "STADO_RESOLVED_EXECUTABLE\t";

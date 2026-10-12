//! Which command families the Desktop API may reach, and which requests
//! only read.
//!
//! The families are the top-level commands of the CLI's own clap tree, so a
//! command the binary has is reachable and one it does not have is refused
//! by the parser's own grammar. Whether one request only reads is judged by
//! a model through Oko's one-shot call (`read-only-prompt.md` beside this
//! file), from the command line and the command's own help — the sentence
//! that says what `--apply` or `--dry-run` does. A hand-kept table of verb
//! words decided this before, and drifted from the tree it described: a new
//! command was classified by whichever arm its words happened to match.
//!
//! The distinction is load-bearing: a command that only reads runs on the
//! Desktop's own request, and anything else needs the explicit
//! `RUN_MUTATION` confirmation the review dialog supplies. A judgement
//! nobody could obtain is a change, said with its cause: the operator is
//! asked to confirm rather than a command run unconfirmed.

use std::sync::{LazyLock, Mutex};

use clap::error::ErrorKind;
use clap::CommandFactory;

use crate::primitives::model_judgement;

const PROMPT: &str = include_str!("read-only-prompt.md");
const ARGS_SLOT: &str = "{args}";
const HELP_SLOT: &str = "{help}";
const READ_VERDICT: &str = "READ";

/// The CLI's own command tree, built once and kept for the process.
static TREE: LazyLock<clap::Command> = LazyLock::new(crate::cli::Cli::command);

/// Every top-level command the CLI declares.
pub(super) static ALLOWED_FAMILIES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    TREE.get_subcommands()
        .map(clap::Command::get_name)
        .collect()
});

/// The last request judged and its verdict: the route asks once to decide
/// and once to report, and both answers must be the same one.
static LAST: Mutex<Option<(Vec<String>, bool)>> = Mutex::new(None);

/// The deepest command `args` names in the tree, with its full help.
fn named_help(args: &[String]) -> String {
    let mut command = TREE.clone();
    for arg in args {
        match command.find_subcommand(arg) {
            Some(sub) => command = sub.clone(),
            None => break,
        }
    }
    command.render_long_help().to_string()
}

fn judged(args: &[String]) -> bool {
    let program = TREE.get_name().to_string();
    let parsed = TREE
        .clone()
        .try_get_matches_from(std::iter::once(program).chain(args.iter().cloned()));
    if let Err(error) = &parsed {
        // Help and version only print; the parser says so itself.
        if matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        ) {
            return true;
        }
    }
    let prompt = PROMPT
        .replace(ARGS_SLOT, &args.join(" "))
        .replace(HELP_SLOT, &named_help(args));
    match model_judgement::ask(&prompt) {
        Ok(answer) => answer.trim().starts_with(READ_VERDICT),
        Err(cause) => {
            eprintln!(
                "[dashboard] `stado {}` is treated as a change: whether it only reads could not be judged: {cause}",
                args.join(" ")
            );
            false
        }
    }
}

pub(super) fn is_read_only(args: &[String]) -> bool {
    let mut last = match LAST.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((held, verdict)) = last.as_ref() {
        if held.as_slice() == args {
            return *verdict;
        }
    }
    let verdict = judged(args);
    *last = Some((args.to_vec(), verdict));
    verdict
}

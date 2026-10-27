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
/// The schema's name and the two options the model chooses between.
const JUDGEMENT: &str = "operator_command_effect";
const READ: &str = "READ";
const CHANGES: &str = "CHANGES";

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
    match model_judgement::choose(&prompt, JUDGEMENT, &[READ, CHANGES]) {
        Ok(judged) => judged.choice == READ,
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

/// The refusal for a request whose command words name no command of the
/// CLI's tree, saying which commands that level does declare; `None` when
/// every word up to the first option or positional names one. A Desktop
/// operation written with a path the CLI does not have (`resolver
/// api-reassign` beside the CLI's `resolver api reassign`) used to reach the
/// parser, be judged by a model first, and end as a bare usage error; it is
/// now refused before either, naming what the CLI offers instead.
pub(super) fn undeclared_path(args: &[String]) -> Option<String> {
    let mut command: &clap::Command = &TREE;
    let mut named: Vec<&str> = Vec::new();
    for arg in args {
        // Options and the arguments of a command that takes positionals are
        // the parser's to judge.
        let takes_positionals = command.get_positionals().next().is_some();
        if arg.starts_with('-')
            || !command.has_subcommands()
            || (takes_positionals && !command.is_subcommand_required_set())
        {
            return None;
        }
        match command.find_subcommand(arg) {
            Some(sub) => {
                named.push(arg.as_str());
                command = sub;
            }
            None => {
                let offered: Vec<&str> = command
                    .get_subcommands()
                    .filter(|sub| !sub.is_hide_set())
                    .map(clap::Command::get_name)
                    .collect();
                let at = match named.as_slice() {
                    [] => "stado".to_string(),
                    words => format!("stado {}", words.join(" ")),
                };
                return Some(format!(
                    "{at} has no command {arg:?}; it declares: {}",
                    offered.join(", ")
                ));
            }
        }
    }
    None
}

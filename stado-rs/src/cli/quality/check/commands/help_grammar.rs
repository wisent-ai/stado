//! Reading a command's direct subcommands out of Clap's help grammar.

use std::collections::BTreeSet;

/// The section header Clap advertises a command's subcommands under.
const COMMANDS: &str = "Commands:";
/// The subcommand Clap generates for help, which is not a command of the
/// product.
const HELP_COMMAND: &str = "help";

/// Read direct commands from Clap's advertised section. These labels belong
/// to Clap's help grammar, not a behavioral keyword classifier. Other help
/// grammars need the model to identify their direct commands.
///
/// A Clap leaf advertises no `Commands:` section; it is known by Clap's own
/// usage line naming exactly this command and Clap's generated help flag.
/// Read as an unknown grammar, every leaf of a Clap CLI was handed to the
/// model, one gateway call per command of every release.
pub(super) fn clap_children<'a>(help: &'a str, name: &str) -> Option<BTreeSet<&'a str>> {
    let usage = format!("Usage: {name}");
    let clap = help.lines().any(|line| line.starts_with(&usage))
        && help.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("-h, --help") && line.contains("Print help")
        });
    let mut lines = help.lines();
    if !lines.by_ref().any(|line| line == COMMANDS) {
        return clap.then(BTreeSet::new);
    }
    let first = lines.clone().find(|line| !line.trim().is_empty())?;
    let indentation = first.len() - first.trim_start().len();
    let mut children = BTreeSet::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            break;
        }
        if line.len() - line.trim_start().len() == indentation {
            children.extend(
                line.split_whitespace()
                    .next()
                    .filter(|name| *name != HELP_COMMAND),
            );
        }
    }
    Some(children)
}

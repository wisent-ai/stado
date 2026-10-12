//! The two low-cardinality keys a failed command is logged under: which
//! command it was, and the top-level command it belongs to.

/// The dotted id of the command that failed, built from the declared
/// subcommand names only — `cli.host.user.create`, never an argument value,
/// so the field stays a low-cardinality key a log query can group by.
pub(super) fn failure_point(matches: &clap::ArgMatches) -> String {
    let mut point = String::from("cli");
    let mut node = matches;
    while let Some((name, sub)) = node.subcommand() {
        point.push('.');
        point.push_str(name);
        node = sub;
    }
    point
}

/// The coarse key a log query groups an incident by: the declared top-level
/// command (`host` for `stado host user create`), read from the parsed tree.
/// A hand-kept table that sorted command names into dependency axes drifted
/// from the tree it described — a command added later fell into a default
/// bucket — so the key is the command tree's own first level, and the
/// dependency that actually failed is said by the failure's code.
pub(super) fn failure_service(matches: &clap::ArgMatches) -> String {
    match matches.subcommand_name() {
        Some(name) => name.to_string(),
        None => failure_point(matches),
    }
}

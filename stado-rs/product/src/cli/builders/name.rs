use super::{flag, positional, value};
use clap::{Arg, ArgAction, Command};

fn json(command: Command, help: &'static str) -> Command {
    command.arg(flag("json", help))
}

/// `stado product name`: the one place a product's names are decided.
pub fn command() -> Command {
    Command::new("name")
        .about(
            "Decide what products are called: read every product's id, display name, the decision \
             that gave them and its former names; say which product a name belongs to and whether \
             a name is free; record a new name with who decided it and why and carry it through \
             the catalog and this machine's receipts",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(json(
            Command::new("list").about(
                "Every product's id and display name, the decision that gave them (who, when, why) \
                 and every name it had before",
            ),
            "Print every product's names as JSON",
        ))
        .subcommand(json(
            Command::new("show")
                .about("The product a name belongs to, current or former, and its naming record")
                .arg(positional("NAME").required(true).help("An id or display name, current or former")),
            "Print the answer as JSON",
        ))
        .subcommand(json(
            Command::new("check")
                .about(
                    "Whether a name is free: no product's id or display name, current or former; \
                     with --org, no repository of the organization named for it. Exits 1 when it \
                     is taken",
                )
                .arg(positional("NAME").required(true).help("The name to check"))
                .arg(flag("as-id", "Also refuse a name that cannot be a product id (lowercase letters, digits and inner hyphens)"))
                .arg(value("org", "Also read this GitHub organization's repositories for one named for the name or <name>-<surface>")),
            "Print whether the name is free and what holds it as JSON",
        ))
        .subcommand(json(
            Command::new("set")
                .about(
                    "Record a naming decision for one product: its new id, its new display name or \
                     both, with who decided and why. The names it had become a former name; a new \
                     id is carried into every catalog record that names the product, the service \
                     label, directory key and release name it still answers to are kept under the \
                     former id, this machine's receipts move to the new id, and every repository, \
                     release manifest and site that still carries the former id is named with what \
                     moves it",
                )
                .arg(positional("PRODUCT").required(true).help("The product's current id"))
                .arg(value("id", "The product's new id: lowercase letters, digits and inner hyphens"))
                .arg(value("name", "The product's new display name"))
                .arg(value("decided-by", "Who decided the name: the operator, or the session that carried his decision").required(true))
                .arg(value("reason", "Why the product takes this name, in the words that decided it").required(true))
                .arg(value("org", "Also read this GitHub organization for repositories named for the former id that no record claims")),
            "Print the decision, every record it changed, what it kept and what still carries the former id as JSON",
        ))
        .subcommand(json(
            Command::new("forget")
                .about(
                    "Remove a former name recorded in error. A name the product really had stays: \
                     it is how a reader of an older page finds the product",
                )
                .arg(positional("PRODUCT").required(true).help("The product's current id"))
                .arg(
                    Arg::new("former")
                        .value_name("FORMER")
                        .required(true)
                        .action(ArgAction::Set)
                        .help("The former id or display name to remove"),
                ),
            "Print the rows removed as JSON",
        ))
        .subcommand(json(
            Command::new("adopt").about(
                "Move every installation receipt this machine keeps under a former product id to \
                 the product's current id, as the catalog's former names say; product sync does \
                 the same on every pass",
            ),
            "Print every receipt moved as JSON",
        ))
}

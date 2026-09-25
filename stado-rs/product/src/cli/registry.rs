use super::{flag, positional, value};
use clap::{ArgAction, ArgGroup, Command};

fn fields(command: Command, required: bool) -> Command {
    command
        .arg(value("name", "Product display name").required(required))
        .arg(value("owner-repository", "Canonical GitHub owner/repository").required(required))
        .arg(value("description", "What the product does").required(required))
        .arg(
            value("visibility", "Product visibility")
                .value_parser(["private", "public"])
                .required(required),
        )
        .arg(value("family", "Product family").required(required))
        .arg(value("status", "Product lifecycle status").required(required))
        .arg(value("evidence", "Canonical source reference; repeatable").action(ArgAction::Append))
        .arg(
            value(
                "surface",
                "Surface declaration KIND=OWNER/REPOSITORY; repeatable",
            )
            .action(ArgAction::Append),
        )
        .arg(
            value("installation", "Installation declaration; repeatable").action(ArgAction::Append),
        )
        .arg(
            value("integration", "Directional product integration; repeatable")
                .action(ArgAction::Append),
        )
        .arg(value(
            "service-file",
            "Service declaration YAML or JSON file",
        ))
        .arg(value(
            "approved-by",
            "Actual operator who approved this product",
        ))
        .arg(value("approval-note", "Recorded approval provenance"))
        .arg(flag(
            "json",
            "Print the registry mutation and persisted record",
        ))
}

pub fn command() -> Command {
    Command::new("registry")
        .about("Mutate the authoritative product registry without replacing neighboring records")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            fields(Command::new("add").about("Register one product"), true)
                .arg(value("id", "New product identifier").required(true)),
        )
        .subcommand(
            fields(
                Command::new("set").about("Update one existing product"),
                false,
            )
            .arg(positional("PRODUCT").required(true))
            .arg(value("add-surface", "Add or replace a surface").action(ArgAction::Append))
            .arg(value("remove-surface", "Remove the named surface").action(ArgAction::Append))
            .arg(
                value("add-installation", "Add or replace an installation")
                    .action(ArgAction::Append),
            )
            .arg(
                value(
                    "remove-installation",
                    "Remove the named installation surface",
                )
                .action(ArgAction::Append),
            )
            .arg(
                value("add-integration", "Add or replace an integration").action(ArgAction::Append),
            )
            .arg(
                value("remove-integration", "Remove the named integration product")
                    .action(ArgAction::Append),
            ),
        )
        .subcommand(
            Command::new("rm")
                .about("Remove one product from the authority")
                .arg(positional("PRODUCT").required(true))
                .arg(flag(
                    "yes",
                    "Authorize this registry removal without a terminal prompt",
                ))
                .arg(flag("json", "Print the mutation result")),
        )
}

pub fn creation() -> Command {
    Command::new("create")
        .about("Provision private repositories and a preview product through Wisent Integrations")
        .arg(value("request", "Creation request JSON file"))
        .arg(value(
            "status",
            "Read the durable result for this request ID",
        ))
        .arg(value("resume", "Resume this exact durable request ID"))
        .group(
            ArgGroup::new("operation")
                .args(["request", "status", "resume"])
                .required(true),
        )
        .arg(flag(
            "allow-create",
            "Authorize the requested private repository creation",
        ))
        .arg(flag("json", "Print the durable provisioning state"))
}

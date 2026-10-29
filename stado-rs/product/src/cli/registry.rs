use super::{flag, positional, value};
use clap::{ArgAction, ArgGroup, Command};

fn fields(command: Command, required: bool) -> Command {
    command
        .arg(value("name", "Product display name").required(required))
        .arg(value("owner-repository", "Canonical GitHub owner/repository").required(required))
        .arg(value("description", "What the product does").required(required))
        .arg(value("visibility", "Product visibility: public or private").required(required))
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
            )
            .arg(
                value(
                    "add-rival",
                    "Add or replace a rival: {\"id\",\"name\",\"url\",\"evidence\"} as JSON",
                )
                .action(ArgAction::Append),
            )
            .arg(value("remove-rival", "Remove the rival with this id").action(ArgAction::Append))
            .arg(
                value(
                    "add-roadmap",
                    "Add or replace a roadmap item: {\"title\",\"status\",\"outcome\",\"source\"} as JSON; status planned, in_progress, delivered, rejected or superseded; an ended item also names \"decision\" (where it was decided) and a superseded one \"superseded_by\" (the title that replaced it)",
                )
                .action(ArgAction::Append),
            )
            .arg(
                value(
                    "remove-roadmap",
                    "Remove the roadmap item with this title: only for an item recorded in error; record a dropped or replaced idea as rejected or superseded instead",
                )
                .action(ArgAction::Append),
            )
            .arg(value(
                "benchmark",
                "Declare the benchmark that measures the rivals: {\"app\",\"suites\"} as JSON",
            ))
            .arg(flag("remove-benchmark", "Remove the declared benchmark"))
            .arg(
                value(
                    "add-dependency",
                    "Add or replace a product this one needs, and how it runs without it: {\"product\",\"surface\",\"for_surface\",\"alternative\",\"alternative_source\"} or {\"product\",\"surface\",\"for_surface\",\"bound_because\"} as JSON; replaces the need with the same product, surface and for_surface",
                )
                .action(ArgAction::Append),
            )
            .arg(
                value(
                    "remove-dependency",
                    "Remove every declared need on the named product; repeatable, and refused when this product declares none on it",
                )
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
        .subcommand(
            Command::new("check-claims")
                .about("Fetch the source of every rival claim of one product again: current, changed since it was read, or unreachable with the cause")
                .arg(positional("PRODUCT").required(true))
                .arg(flag("json", "Print every claim's state")),
        )
}

pub fn creation() -> Command {
    Command::new("create")
        .about("Provision private repositories and a preview product through Wisent Integrations")
        .arg(value("request", "Creation request JSON file"))
        .arg(
            value(
                "plan",
                "Check this creation request JSON file with reads only and print what create \
                 would provision and register; creates nothing and needs no --allow-create",
            )
            .conflicts_with("allow-create"),
        )
        .arg(value(
            "status",
            "Read the durable result for this request ID",
        ))
        .arg(value("resume", "Resume this exact durable request ID"))
        .group(
            ArgGroup::new("operation")
                .args(["request", "plan", "status", "resume"])
                .required(true),
        )
        .arg(flag(
            "allow-create",
            "Authorize the requested private repository creation",
        ))
        .arg(flag(
            "json",
            "Print the durable provisioning state; with --plan, the plan as JSON",
        ))
}

/// `stado product surface add`: one more repository surface for a product
/// the catalog holds, provisioned the way `create` provisions each one.
pub fn surface_add() -> Command {
    Command::new("add")
        .about(
            "Provision one private repository for an existing catalog product and record it \
             as that product's surface of KIND, the way create provisions each repository; \
             run it again to resume one that stopped",
        )
        .arg(
            positional("PRODUCT")
                .required(true)
                .help("An existing catalog product"),
        )
        .arg(
            value(
                "kind",
                "Surface kind: cli, desktop, web, service, ios or android",
            )
            .required(true)
            .value_parser(|word: &str| {
                crate::creation::Surface::parse(word).map(|_| word.to_owned())
            }),
        )
        .arg(value("repository", "The new repository, OWNER/NAME").required(true))
        .arg(
            flag(
                "plan",
                "Check with reads only and print what would be provisioned and recorded; \
                 creates nothing",
            )
            .conflicts_with("allow-create"),
        )
        .arg(flag(
            "allow-create",
            "Authorize the private repository creation",
        ))
        .arg(flag(
            "json",
            "Print the plan or the provisioning result as JSON",
        ))
}

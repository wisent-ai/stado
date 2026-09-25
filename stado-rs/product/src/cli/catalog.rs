use super::{flag, value};
use clap::{ArgGroup, Command};

pub fn command() -> Command {
    Command::new("catalog")
        .about("Read and validate the authoritative Wisent product catalog")
        .arg(flag("json", "Print the product catalog as JSON"))
        .arg(
            flag(
                "cli-products",
                "Derive the CLI product and documentation inventory",
            )
            .conflicts_with_all([
                "check-package",
                "check-repositories",
                "unclaimed",
                "unapproved",
            ]),
        )
        .arg(value("output", "Write the derived catalog to this path"))
        .arg(value(
            "check",
            "Compare this existing derived catalog with the authority",
        ))
        .arg(flag(
            "check-package",
            "Compare the independent authority with this binary's embedded catalog",
        ))
        .arg(flag(
            "check-repositories",
            "Verify declared repository identities through GitHub",
        ))
        .arg(
            value(
                "unclaimed",
                "List active organization repositories absent from the authority",
            )
            .num_args(0..=1)
            .default_missing_value("wisent-ai"),
        )
        .arg(flag(
            "unapproved",
            "List product records without operator approval provenance",
        ))
        .group(ArgGroup::new("action").args([
            "output",
            "check",
            "check-package",
            "check-repositories",
            "unclaimed",
            "unapproved",
        ]))
}

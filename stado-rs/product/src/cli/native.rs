use super::{flag, value};
use clap::{Arg, Command};

fn forwarded() -> Arg {
    Arg::new("forward")
        .value_name("ARGUMENTS")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .help("Arguments passed to the real compiler after the operation")
}

pub fn cargo() -> Command {
    Command::new("cargo")
        .about("Run Cargo against canonical source checkouts with an isolated lockfile")
        .arg(value(
            "manifest-path",
            "Canonical Cargo.toml; defaults to the current directory",
        ))
        .arg(flag("json", "Print retained source and execution evidence"))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["build", "check", "test", "run", "metadata"]),
        )
        .arg(forwarded())
}

pub fn swift() -> Command {
    Command::new("swift")
        .about("Build and index canonical Swift sources, or serve recorded SourceKit settings")
        .arg(value(
            "package-path",
            "Canonical package directory; defaults to the current directory",
        ))
        .arg(value(
            "editor-workspace",
            "Separate editor workspace for index declarations",
        ))
        .arg(flag("json", "Print retained source and execution evidence"))
        .arg(Arg::new("operation").required(true).value_parser([
            "build",
            "test",
            "run",
            "index",
            "sourcekit",
        ]))
        .arg(forwarded())
}

pub fn documentation() -> Command {
    Command::new("documentation")
        .about("Check actual published documentation and GitHub repository policy")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("cli-pages")
                .about("Verify linked command pages and canonical URLs on product websites")
                .arg(
                    value(
                        "origin",
                        "Verify this catalogued documentation origin; repeatable",
                    )
                    .action(clap::ArgAction::Append),
                ),
        )
        .subcommand(
            Command::new("markdown-policy")
                .about("Find repository Markdown beyond the root README.md")
                .arg(value("org", "GitHub organization; defaults to wisent-ai"))
                .arg(flag(
                    "include-archived",
                    "Inspect archived repositories as well as active ones",
                )),
        )
}

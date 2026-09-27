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
                .value_parser(["build", "check", "test", "run", "metadata", "stage"])
                .help(
                    "stage: a locked release build of the forwarded --bin targets, each \
                     binary placed at $WISENT_OUTPUT_DIR/<name> for a release manifest to stage",
                ),
        )
        .arg(forwarded())
}

pub fn source_bundle() -> Command {
    Command::new("source-bundle").about(
        "Release build step: the checkout's files in a reproducible \
         $WISENT_OUTPUT_DIR/release/source-bundle.tar with each file's digest",
    )
}

pub fn python() -> Command {
    Command::new("python")
        .about("Release steps of a Python package: build its wheel and sdist, or upload them to PyPI")
        .arg(Arg::new("operation").required(true).value_parser(["build", "deliver-pypi"]).help(
            "build: python -m build into $WISENT_OUTPUT_DIR/release/python-distributions.tar; \
             deliver-pypi: verify WISENT_RELEASE_ARCHIVE against WISENT_RELEASE_SHA256 and \
             upload its one wheel and one sdist with twine, writing pypi-evidence.json",
        ))
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

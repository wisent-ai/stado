use super::{flag, positional, value};
use clap::{Arg, ArgAction, ArgGroup, Command};

fn surface(required: bool) -> Arg {
    value("surface", "Product installation surface")
        .value_parser(["cli", "desktop", "service"])
        .required(required)
}

pub fn installation(action: &'static str, about: &'static str) -> Command {
    let command = Command::new(action)
        .about(about)
        .arg(positional("PRODUCT").required(true))
        .arg(surface(true))
        .arg(value("host", "Stado host for a service surface"))
        .arg(flag("json", "Print lifecycle state and observed readiness"));
    if action == "install" || action == "update" {
        command
            .arg(
                value(
                    "release-version",
                    "Install this exact qualified release version",
                )
                .requires("source-commit"),
            )
            .arg(
                value(
                    "source-commit",
                    "Full source commit bound to the qualified release",
                )
                .requires("release-version"),
            )
    } else {
        command
    }
}

pub fn sync() -> Command {
    Command::new("sync")
        .about("Reconcile installed surfaces against canonical origin/main")
        .arg(surface(true))
        .arg(value("host", "Stado host for service surfaces"))
        .arg(flag(
            "fetch",
            "Fetch canonical origins before checking source readiness",
        ))
        .arg(flag(
            "dry-run",
            "Report decisions without fetching, building or installing",
        ))
        .arg(flag(
            "json",
            "Print each product's final decision and error",
        ))
}

pub fn schedule() -> Command {
    Command::new("schedule")
        .about("Inspect or reconcile the native macOS product update agents")
        .arg(
            flag(
                "install",
                "Install or update the declared native launch agents",
            )
            .conflicts_with("remove"),
        )
        .arg(flag("remove", "Remove the declared product update agents"))
        .arg(flag(
            "json",
            "Print launchd observations and reconciliation results",
        ))
}

pub fn signing() -> Command {
    Command::new("signing").about("Inspect and preserve stable Apple native code identity")
        .subcommand_required(true).arg_required_else_help(true)
        .subcommand(Command::new("inspect").about("Inspect actual signatures on files or bundles")
            .arg(positional("PATH").num_args(1..).required(true)).arg(flag("json", "Print signature observations"))
            .arg(flag("entitlements", "Read the actual signed entitlement dictionary")))
        .subcommand(Command::new("sign").about("Sign staged code and atomically replace only verified targets")
            .arg(positional("PATH").num_args(1..).required(true))
            .arg(value("product", "Product whose stable code identity is preserved"))
            .arg(value("identifier", "Exact identifier for one signing target"))
            .group(ArgGroup::new("code-identity").args(["product", "identifier"]).required(true).multiple(true))
            .arg(value("identity", "Exact available Apple signing certificate name or fingerprint"))
            .arg(value("previous", "Preceding stable installation whose requirement must remain satisfied"))
            .arg(value("entitlements", "Exact entitlement plist for the selected target; nested code retains its own policy"))
            .arg(value("boolean-entitlement", "Merge NAME=true or NAME=false into each target's entitlements; repeatable")
                .action(ArgAction::Append))
            .arg(flag("hardened-runtime", "Select the hardened runtime signing option"))
            .arg(flag("json", "Print the resulting signatures")))
        .subcommand(Command::new("report").about("Read signatures of a product's recorded installation")
            .arg(positional("PRODUCT").required(true)).arg(surface(false)).arg(flag("json", "Print signature observations")))
        .subcommand(Command::new("reconcile").about("Repair unstable signatures on recorded installed paths")
            .arg(positional("PRODUCT").required(true)).arg(surface(false)).arg(flag("json", "Print resulting signatures")))
        .subcommand(Command::new("residue").about("Report obsolete signed installer staging paths")
            .arg(value("root", "Root to inspect; repeatable").action(ArgAction::Append)).arg(flag("json", "Print residue observations")))
        .subcommand(Command::new("stage").about("Sign only the native files declared in a release manifest")
            .arg(value("manifest", "Release manifest path").required(true))
            .arg(value("output", "Prepared release output directory").required(true))
            .arg(value("platform", "Declared release platform").required(true))
            .arg(flag("json", "Print verified stage signatures")))
}

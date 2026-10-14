use super::{flag, positional, value};
use anyhow::Context;
use clap::{Arg, ArgAction, ArgGroup, Command};

fn surface(required: bool) -> Arg {
    value("surface", "Product installation surface")
        .value_parser(["cli", "desktop", "service"])
        .required(required)
}

/// What a lifecycle verb does, stated by the caller that declares the verb,
/// so the arguments it takes follow from that and not from its name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Reads state only; never waits for or supersedes a writer.
    Reads,
    /// Changes an installation without building it.
    Changes,
    /// Builds or places release bytes: takes release coordinates.
    Builds,
    /// Builds a new source or release, or projects its provider-first plan.
    BuildsWithPlan,
}

pub fn installation(action: &'static str, about: &'static str, shape: Shape) -> Command {
    let command = Command::new(action)
        .about(about)
        .arg(positional("PRODUCT").required(true))
        .arg(surface(true))
        .arg(value("host", "Stado host for a service surface"))
        .arg(flag("json", "Print lifecycle state and observed readiness"));
    let command = if shape == Shape::Reads {
        command
    } else {
        command.arg(flag(
            "wait",
            "Run after another process changing this surface instead of superseding it: \
             without --wait a holder still preparing is stopped and replaced, a holder \
             already placing files is waited for",
        ))
    };
    let command = if matches!(shape, Shape::Builds | Shape::BuildsWithPlan) {
        command
            .arg(
                value(
                    "release-version",
                    "Install this exact qualified release version",
                )
                .requires("source-commit"),
            )
            .arg(value(
                "source-commit",
                "Full source commit. With --release-version, the commit bound to that \
                 qualified release; alone, the exact canonical commit a source build \
                 installs, whatever origin/main has moved to since",
            ))
            .arg(value(
                "without",
                "Comma-separated products this machine does without; a dependency on one \
                 is skipped only when the catalogue declares its alternative, and the \
                 choice is kept for later updates",
            ))
            .arg(value(
                "signing-keychain",
                "Sign darwin native code with an identity from this local keychain \
                 (WISENT_CODESIGN_KEYCHAIN for this invocation). A catalog recipe's \
                 signing_keychain requires that exact keychain; a role or PEM pair cannot \
                 replace it. The fleet signing role is not read",
            ))
            .arg(value(
                "signing-identity",
                "The signing identity, by its name or SHA-1 (WISENT_CODESIGN_IDENTITY for this \
                 invocation): required to change the identity the installed code is signed by, \
                 or to pick one from a keychain holding several; the report names the change",
            ))
            .arg(flag(
                "check-arguments",
                "Check this command line (required pairs such as --release-version with \
                 --source-commit, the product and the surface) and exit without installing",
            ))
    } else {
        command
    };
    if shape == Shape::BuildsWithPlan {
        command
            .arg(flag(
                "plan",
                "Print the ordered prerequisite and target source revisions, installed state and reason without installing",
            ))
            .arg(flag(
                "with-prerequisites",
                "Install missing declared verbs from each provider's qualified last-green source revision before installing this product",
            ))
    } else {
        command
    }
}

/// The verbs that read or change one installation without building it:
/// `status` (with the operation holding its lock), `cancel` (ends that
/// operation through its owner), `remove` and `rollback`.
pub fn changes() -> Vec<Command> {
    vec![
        installation(
            "status",
            "Inspect recorded installation, source, signatures, actual readiness and the operation \
             holding its lock, with its step",
            Shape::Reads,
        ),
        installation(
            "cancel",
            "End the installation operation holding a product surface's lock: its owner and the \
             step it runs are told to stop, the lock is waited for until it is let go, and the \
             receipt records the cancelled operation and its step",
            Shape::Changes,
        ),
        installation("remove", "Remove only owned paths and the declared service surface", Shape::Changes),
        installation("rollback", "Restore exact retained installation bytes without rebuilding", Shape::Changes),
    ]
}

pub fn locks() -> Command {
    Command::new("locks")
        .about("List every product surface lock, its recorded owner, process identity, observed state and corruption")
        .arg(flag("json", "Print the complete lock observations as JSON"))
}

pub fn report_locks(runtime: &crate::common::Runtime, as_json: bool) -> anyhow::Result<i32> {
    let report = crate::common::files::locks_report(&runtime.home)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for row in report["locks"]
            .as_array()
            .context("lock report has no rows")?
        {
            let path = row["path"].as_str().context("lock row has no path")?;
            if let Some(error) = row["error"].as_str() {
                println!("{path}: unreadable or corrupt: {error}");
                continue;
            }
            let state = match (row["held"].as_bool(), row["running"].as_bool()) {
                (Some(true), Some(true)) => "held",
                (Some(true), _) => "held with unverified process identity",
                (_, Some(true)) => "released record",
                _ => "stale record",
            };
            println!(
                "{path}: {state}; pid {}, born {}, observed {}; phase {}, step {}; acquired {}",
                row["pid"],
                row["started_at"],
                row["observed_start"],
                row["phase"],
                row["step"],
                row["acquired_at"]
            );
        }
    }
    Ok(libc::EXIT_SUCCESS)
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
            "clone-missing",
            "Clone a product's source from its GitHub origin into the workspace when the \
             workspace holds no checkout of it, instead of holding the product",
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
        .subcommand(Command::new("notarize")
            .about("Notarize a signed .app with Apple's notary (AC_API_KEY_ID, AC_API_ISSUER_ID, AC_API_KEY_P8), staple it and check Gatekeeper; the notary's answer goes to --evidence, default $WISENT_OUTPUT_DIR/notary.json")
            .arg(value("app", "Signed .app bundle to notarize").required(true))
            .arg(value("evidence", "Where the notary's JSON answer is written"))
            .arg(flag("json", "Print the notarization result")))
}

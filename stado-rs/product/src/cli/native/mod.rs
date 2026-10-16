use super::{flag, value};
use clap::{Arg, Command};

mod delivery;
pub use delivery::{deliver, schema};

pub(super) fn forwarded() -> Arg {
    Arg::new("forward")
        .value_name("ARGUMENTS")
        .num_args(..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .help("Arguments passed to the real compiler after the operation")
}

/// Editor protocols use the source tree's native build settings, not a
/// compiler-specific command name.
pub fn editor() -> Command {
    Command::new("editor")
        .about("Serve editor build settings or inspect live workspace instances")
        .arg(value(
            "package-path",
            "Canonical package directory; serve may use the initialization URI, status lists all when omitted",
        ))
        .arg(flag("json", "Print observed editor instances as JSON; status only"))
        .arg(Arg::new("operation").required(true).value_parser(["serve", "status"]))
}

pub fn cargo() -> Command {
    Command::new("cargo")
        .about("Run Cargo against canonical checkouts or declared release-worker source inputs")
        .arg(value(
            "manifest-path",
            "Canonical Cargo.toml; defaults to the current directory",
        ))
        .arg(value(
            "revision",
            "Build this committed revision of the manifest's repository instead of its working \
             tree: the commit is exported once into the checkout's build area \
             (~/.stado/products/builds/<checkout below home>/stado-build/<commit>/) and read and built there with the same \
             locked flags and compiler cache, so another session's uncommitted edit in the \
             checkout is not read; a commit the checkout does not hold is refused",
        ))
        .arg(flag("all", "Read retained Cargo receipts across canonical workspace checkouts; results only"))
        .arg(flag("json", "Print retained source and execution evidence"))
        .arg(flag(
            "wait",
            "When the disk janitor has asked running work for its turn, wait for that work, run \
             the janitor's pass and then build, instead of being refused before anything is \
             exported or built",
        ))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["build", "check", "test", "run", "metadata", "stage", "results"])
                .help(
                    "stage: a locked release build of the forwarded --bin targets, each \
                     binary placed at $WISENT_OUTPUT_DIR/<name>; results: retained Cargo receipts for this checkout, with --json",
                ),
        )
        .arg(forwarded())
}
/// The operations `stado product cargo` runs on any tree: the values its
/// declaration names for `operation`.
pub(crate) fn cargo_operations() -> Vec<String> {
    cargo()
        .get_arguments()
        .filter(|argument| argument.get_id() == "operation")
        .flat_map(Arg::get_possible_values)
        .map(|value| value.get_name().to_string())
        .collect()
}

/// `stado product cargo` with its operation open beyond the declared values:
/// a Cargo subcommand a quality gate of the built tree declares is admitted
/// too, once the tree is known (`cargo::declared::admit`).
///
/// Clap's `mut_arg` takes the argument out and appends it again, so the
/// reopened `operation` would follow `forward` and the trailing arguments
/// would no longer be the last positional: every debug build refused the
/// command definition (`cargo:forward: Arg::trailing_var_arg can only apply
/// to last positional`) before parsing a word. `forward` is appended after it
/// again, unchanged.
pub(super) fn cargo_command() -> Command {
    cargo()
        .mut_arg("operation", |argument| {
            argument
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .long_help(
                    "build, check, test, run, metadata or stage on any tree, or any Cargo \
                     subcommand a quality gate of the built tree's .wisent-release.json runs \
                     (clippy; not fmt, which compiles nothing): read under --revision from that \
                     committed revision. stage: \
                     a locked release build of the forwarded --bin targets, each binary placed \
                     at $WISENT_OUTPUT_DIR/<name> for a release manifest to stage",
                )
        })
        .mut_arg("forward", |argument| argument)
}

/// The compiler cache every Cargo build Stado runs goes through.
pub fn compiler_cache() -> Command {
    Command::new("compiler-cache")
        .about("Report, install or remove the compiler cache (Kache) every Cargo build Stado runs goes through")
        .arg(flag("json", "Print the observed state as JSON"))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["status", "ensure", "remove"])
                .help(
                    "status: the installed version against the declared one; ensure: install the \
                     declared version with cargo install when it is absent or another; remove: \
                     cargo uninstall it",
                ),
        )
}

/// The rebuildable trees in the workspace's checkouts, which the janitor may
/// not read when the checkouts sit in a folder macOS keeps from it.
pub fn build_trees() -> Command {
    Command::new("build-trees")
        .about(
            "List or remove the rebuildable trees in the workspace's checkouts — CACHEDIR.TAG \
             directories, Stado's former install runs, desktop .build — from this process",
        )
        .arg(flag(
            "json",
            "Print every tree, its bytes and its state as JSON",
        ))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["list", "remove"])
                .help(
                    "list: every rebuildable tree with its bytes and whether a build holds it; \
                     remove: delete every one no build holds and report the bytes",
                ),
        )
}

/// The Node.js every Node product build Stado runs uses.
pub fn node_runtime() -> Command {
    Command::new("node-runtime")
        .about("Report or install the Node.js release every Node product build Stado runs uses")
        .arg(flag("json", "Print the observed state as JSON"))
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["status", "ensure"])
                .help(
                    "status: whether the declared release is installed and answers; ensure: \
                     download it from Node's distribution, verify it against SHASUMS256.txt and \
                     link its programs into ~/.local/bin when it is absent or another",
                ),
        )
}

pub fn source_bundle() -> Command {
    Command::new("source-bundle")
        .about(
            "Release build step: the checkout's files in a reproducible \
             $WISENT_OUTPUT_DIR/release/<name> with each file's digest, and \
             release/SOURCE_REVISION from $WISENT_SOURCE_COMMIT when it is set",
        )
        .arg(
            Arg::new("name")
                .long("name")
                .default_value("source-bundle.tar")
                .help("The bundle's file name under release/"),
        )
        .arg(
            Arg::new("include")
                .long("include")
                .action(clap::ArgAction::Append)
                .help("Bundle only this checkout path (file or directory); repeatable"),
        )
}

pub fn python() -> Command {
    Command::new("python")
        .about("Release steps of a Python product: its wheel and sdist, their PyPI upload, or a zip application")
        .arg(Arg::new("operation").required(true).value_parser(["build", "deliver-pypi", "zipapp"]).help(
            "build: python -m build into $WISENT_OUTPUT_DIR/release/python-distributions.tar; \
             deliver-pypi: verify WISENT_RELEASE_ARCHIVE against WISENT_RELEASE_SHA256 and \
             upload its one wheel and one sdist with twine, writing pypi-evidence.json; \
             zipapp: the --package directories run as --module, at $WISENT_OUTPUT_DIR/bin/<--name>",
        ))
        .arg(Arg::new("package").long("package").action(clap::ArgAction::Append).help("zipapp: a top-level package directory; repeatable"))
        .arg(Arg::new("module").long("module").help("zipapp: the module the application runs"))
        .arg(Arg::new("name").long("name").help("zipapp: the application's file name, ending in .pyz"))
}

pub fn npm() -> Command {
    Command::new("npm")
        .about("Release steps of an npm package")
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["pack"])
                .help(
                    "pack: npm pack --ignore-scripts of the checkout into \
             $WISENT_OUTPUT_DIR/release/npm-package.tgz, with npm-package.tgz.sha256",
                ),
        )
}

pub fn surface() -> Command {
    Command::new("surface")
        .about("Refuse a Swift package revision whose version tag disagrees with what it did to the library's public API, as swift api-digester reads it against the released tag")
        .arg(value(
            "package-path",
            "Package directory; defaults to the current directory",
        ))
        .arg(value(
            "module",
            "The library module to read; defaults to the manifest's single library product",
        ))
        .arg(flag("json", "Print the dumps, the digester's diagnosis and the verdict"))
}

pub fn documentation() -> Command {
    Command::new("documentation")
        .about("Check actual published documentation and GitHub repository policy, or generate a documentation site's search index")
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
        .subcommand(
            Command::new("index")
                .about(
                    "Generate search-index.json and the docs/index.html topic cards of a static \
                     documentation site from the pages its docs-manifest.json lists; --check \
                     writes nothing and fails when either file is stale",
                )
                .arg(value("root", "The site checkout; defaults to the current directory"))
                .arg(flag("check", "Refuse stale files instead of writing them")),
        )
}

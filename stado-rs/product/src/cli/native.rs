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

pub fn supabase() -> Command {
    Command::new("supabase")
        .about("Release steps of a Supabase schema product")
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["verify"])
                .help(
                    "verify: the post-build test of a supabase-source platform; applies every \
             migration of $WISENT_OUTPUT_DIR/release/supabase-source.tar to a scratch local \
             database (supabase db start, Supabase CLI and Docker on the runner) and stops it",
                ),
        )
        .arg(project_dir())
}

/// Where the Supabase project (the directory holding `supabase/`) sits inside
/// the bundle, for a repository that keeps it below its root (`web`).
fn project_dir() -> Arg {
    Arg::new("project-dir")
        .long("project-dir")
        .default_value(".")
        .help("Directory inside the bundle that holds supabase/ (default: the bundle root)")
}

pub fn deliver() -> Command {
    Command::new("deliver")
        .about("Release deliveries to hosting providers, run by a manifest's deliveries")
        .subcommand_required(true)
        .subcommand(
            Command::new("supabase")
                .about(
                    "Push the verified release's supabase-source.tar (migrations, functions) to the \
                     Supabase project SUPABASE_PROJECT_REF with SUPABASE_ACCESS_TOKEN and \
                     SUPABASE_DB_PASSWORD, carrying split migrations in as applied; writes \
                     supabase-receipt.json",
                )
                .arg(project_dir()),
        )
        .subcommand(
            Command::new("testflight")
                .about(
                    "Upload the verified release's .ipa to App Store Connect with xcrun altool \
                     (AC_API_KEY_ID, AC_API_ISSUER_ID, AC_API_KEY_P8); writes \
                     testflight-receipt.json",
                )
                .arg(
                    Arg::new("ipa")
                        .long("ipa")
                        .required(true)
                        .help("The release's .ipa file name"),
                ),
        )
        .subcommand(
            Command::new("github-mirror")
                .about(
                    "Mirror the verified release on GitHub (WISENT_MIRROR_TOKEN): tag v<version> \
                     at the release's SOURCE_REVISION, a release titled '<title> <version>', and \
                     the archive as its asset; writes github-mirror-receipt.json",
                )
                .arg(
                    Arg::new("repository")
                        .long("repository")
                        .required(true)
                        .help("OWNER/NAME"),
                )
                .arg(
                    Arg::new("title")
                        .long("title")
                        .required(true)
                        .help("The release title's product name"),
                )
                .arg(Arg::new("signed-binary").long("signed-binary").help(
                    "Refuse unless this archive path carries a valid Developer ID signature",
                )),
        )
        .subcommand(Command::new("npm").about(
            "Publish the verified release's npm-package.tgz unchanged (NPM_TOKEN), running no \
             package scripts; writes npm-receipt.json",
        ))
        .subcommand(
            Command::new("render")
                .about(
                    "Start a deploy of the Render service of this name (RENDER_API_KEY); \
                     writes render-evidence.json with the deploy and the status Render answered",
                )
                .arg(Arg::new("service-name").long("service-name").required(true)),
        )
        .subcommand(
            Command::new("vercel-files")
                .about(
                    "Upload the source/ files of a bundle in the verified release to Vercel \
                     (VERCEL_TOKEN) as a production deployment and wait until it is ready; \
                     writes vercel-evidence.json",
                )
                .arg(Arg::new("bundle").long("bundle").required(true))
                .arg(Arg::new("team-id").long("team-id").required(true))
                .arg(Arg::new("project-id").long("project-id").required(true))
                .arg(Arg::new("project-name").long("project-name").required(true)),
        )
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

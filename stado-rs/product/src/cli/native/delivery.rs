//! `stado product schema` and `stado product deliver`: the commands that
//! apply a product's schema and hand its release to where it runs.

use clap::{Arg, Command};
pub fn schema() -> Command {
    Command::new("schema")
        .about("Release steps of a database-schema product on any engine: sqlite, postgres or supabase")
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["verify"])
                .help(
                    "verify: the post-build test of a schema product; applies every migration, in \
                     file-name order, to a scratch database of the engine and reports the tables it \
                     made. sqlite and postgres read release/database-schema.tar, produced by \
                     source-bundle, never the current checkout. sqlite: a fresh file under \
                     $WISENT_OUTPUT_DIR/schema-verify. postgres: the scratch database \
                     WISENT_SCRATCH_DATABASE_URL names, through psql. supabase: \
                     $WISENT_OUTPUT_DIR/release/supabase-source.tar on a local Supabase \
                     stack (supabase db start, Supabase CLI and Docker on the runner)",
                ),
        )
        .arg(engine())
        .arg(migrations())
        .arg(project_dir())
}

/// The engine the schema runs on; there is no default, because the engine is
/// a fact of the product the manifest declares.
fn engine() -> Arg {
    Arg::new("engine")
        .long("engine")
        .required(true)
        .value_parser(["sqlite", "postgres", "supabase"])
        .help("The engine the schema runs on")
}

/// Where the `*.sql` migrations sit inside the schema source bundle.
fn migrations() -> Arg {
    Arg::new("migrations")
        .long("migrations")
        .default_value("migrations")
        .help("Directory inside database-schema.tar's source/ root; *.sql files run in file-name order (sqlite and postgres)")
}

/// Where the Supabase project (the directory holding `supabase/`) sits inside
/// the bundle, for a repository that keeps it below its root (`web`).
fn project_dir() -> Arg {
    Arg::new("project-dir")
        .long("project-dir")
        .default_value(".")
        .help(
            "Directory inside the bundle that holds supabase/ (supabase; default: the bundle root)",
        )
}

pub fn deliver() -> Command {
    Command::new("deliver")
        .about("Release deliveries to hosting providers, run by a manifest's deliveries")
        .subcommand_required(true)
        .subcommand(
            Command::new("schema")
                .about(
                    "Apply the verified release's migrations to the product's database on its engine. \
                     postgres: verify WISENT_RELEASE_ARCHIVE against WISENT_RELEASE_SHA256, \
                     read its one database-schema.tar, then apply the migrations the database \
                     SCHEMA_DATABASE_URL names has not recorded yet, each with its version \
                     recorded in wisent_schema_migrations; writes schema-receipt.json. \
                     supabase: push supabase-source.tar (migrations, functions) \
                     to the project SUPABASE_PROJECT_REF with SUPABASE_ACCESS_TOKEN and \
                     SUPABASE_DB_PASSWORD, carrying split migrations in as applied. Declared \
                     supabase/function-secrets.json names are resolved from injected environment \
                     values before mutations and published before functions deploy; writes \
                     supabase-receipt.json with secret names, never values. sqlite has no \
                     delivery: the product applies its own migrations on open",
                )
                .arg(engine())
                .arg(migrations())
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

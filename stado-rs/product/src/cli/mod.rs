mod catalog;
mod lifecycle;
mod native;
pub(crate) mod registry;
use crate::common::Runtime;
use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command};
use std::path::{Path, PathBuf};

fn value(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help).num_args(1)
}
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}
fn positional(name: &'static str) -> Arg {
    Arg::new("positional").value_name(name)
}

/// Add the product arguments and operations to Stado's `product` command.
pub fn augment(command: Command) -> Command {
    command
        .arg(value("catalog", "Independent authoritative catalog path").global(true))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(catalog::command())
        .subcommand(registry::command())
        .subcommand(registry::creation())
        .subcommand(lifecycle::installation(
            "install",
            "Install one product surface from its canonical recipe or exact qualified release",
        ))
        .subcommand(lifecycle::installation(
            "update",
            "Update one product surface while retaining exact rollback bytes",
        ))
        .subcommand(lifecycle::installation(
            "status",
            "Inspect recorded installation, source, signatures and actual readiness",
        ))
        .subcommand(lifecycle::installation(
            "remove",
            "Remove only owned paths and the declared service surface",
        ))
        .subcommand(lifecycle::installation(
            "rollback",
            "Restore exact retained installation bytes without rebuilding",
        ))
        .subcommand(lifecycle::sync())
        .subcommand(lifecycle::signing())
        .subcommand(
            Command::new("paths")
                .about("Inspect actual executable ownership and PATH collisions")
                .arg(flag("json", "Print observed path collisions")),
        )
        .subcommand(native::cargo())
        .subcommand(native::source_bundle())
        .subcommand(native::python())
        .subcommand(native::npm())
        .subcommand(native::schema())
        .subcommand(native::deliver())
        .subcommand(
            Command::new("linkage")
                .about("Prove each .app bundle's @rpath dependencies resolve inside it, as dyld resolves them")
                .arg(clap::Arg::new("bundle").required(true).num_args(1..).help("A .app bundle; repeatable")),
        )
        .subcommand(
            Command::new("crx3")
                .about("Pack a browser extension directory as a signed CRX3 and its Omaha update manifest, refusing a key whose extension id is not the pinned one")
                .arg(clap::Arg::new("extension").long("extension").required(true).help("The unpacked extension directory"))
                .arg(clap::Arg::new("key").long("key").required(true).help("The RSA private key (PEM) that signs it"))
                .arg(clap::Arg::new("expected-id").long("expected-id").required(true).help("The extension id the key must produce"))
                .arg(clap::Arg::new("codebase").long("codebase").required(true).help("The URL the update manifest points at"))
                .arg(clap::Arg::new("version").long("version").required(true).help("The version written into manifest.json and the update manifest"))
                .arg(clap::Arg::new("crx").long("crx").required(true).help("Where the .crx is written"))
                .arg(clap::Arg::new("update-manifest").long("update-manifest").required(true).help("Where the update manifest .xml is written")),
        )
        .subcommand(
            Command::new("tree-archive")
                .about("Pack one directory, under its own name, as a reproducible .tar.gz: path order, no owner, time zero, modes 0755/0644, links kept")
                .arg(clap::Arg::new("source").long("source").required(true).help("The directory, e.g. an .xcarchive"))
                .arg(clap::Arg::new("output").long("output").required(true).help("The .tar.gz written")),
        )
        .subcommand(native::swift())
        .subcommand(native::surface())
        .subcommand(native::documentation())
        .subcommand(
            Command::new("changelog")
                .about("Move CHANGELOG.md's Unreleased entries into changelog/ as the given release, in the version-bump commit")
                .arg(clap::Arg::new("version").long("version").required(true).help("The release the entries go out in"))
                .arg(clap::Arg::new("root").long("root").default_value(".").help("The repository checkout")),
        )
}

/// Run one parsed `stado product` invocation as `build`, returning its exit status.
pub fn run(mut matches: clap::ArgMatches, build: crate::Build) -> Result<i32> {
    let _ = crate::BUILD.set(build);
    let runtime = Runtime::new(matches.get_one::<String>("catalog").map(PathBuf::from))?;
    let (action, mut arguments) = matches
        .remove_subcommand()
        .context("product command is missing")?;
    match action.as_str() {
        "catalog" => crate::catalog::run(arguments, &runtime),
        "create" => crate::creation::run(arguments, &runtime),
        "registry" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("registry operation is missing")?;
            crate::registry::run(&action, arguments, &runtime)
        }
        "install" | "update" | "status" | "remove" | "rollback" | "sync" => {
            crate::install::run(&action, arguments, &runtime)
        }
        "signing" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("signing operation is missing")?;
            crate::signing::run(&action, arguments, &runtime)
        }
        "paths" => crate::paths::run(arguments, &runtime),
        "cargo" => crate::cargo::run(arguments, &runtime),
        "source-bundle" => crate::release_steps::run_source_bundle(
            arguments
                .get_one::<String>("name")
                .context("bundle name is missing")?,
            &arguments
                .get_many::<String>("include")
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        ),
        "deliver" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("delivery is missing")?;
            crate::release_steps::run_deliver(&action, &arguments)
        }
        "linkage" => crate::release_steps::run_linkage(
            &arguments
                .get_many::<String>("bundle")
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        ),
        "crx3" => {
            let text = |name: &str| -> Result<String> {
                arguments
                    .get_one::<String>(name)
                    .cloned()
                    .with_context(|| format!("crx3 requires --{name}"))
            };
            crate::release_steps::run_crx3(&crate::release_steps::Crx3Request {
                extension: PathBuf::from(text("extension")?),
                key: PathBuf::from(text("key")?),
                expected_id: text("expected-id")?,
                codebase: text("codebase")?,
                version: text("version")?,
                crx: PathBuf::from(text("crx")?),
                update_manifest: PathBuf::from(text("update-manifest")?),
            })
        }
        "tree-archive" => {
            let path = |name: &str| -> Result<PathBuf> {
                arguments
                    .get_one::<String>(name)
                    .map(PathBuf::from)
                    .with_context(|| format!("tree-archive requires --{name}"))
            };
            crate::release_steps::run_tree_archive(&path("source")?, &path("output")?)
        }
        "npm" => match arguments
            .get_one::<String>("operation")
            .context("npm release operation is missing")?
            .as_str()
        {
            "pack" => crate::release_steps::run_npm_pack(),
            other => anyhow::bail!("unknown npm release operation {other}"),
        },
        "schema" => match arguments
            .get_one::<String>("operation")
            .context("schema release operation is missing")?
            .as_str()
        {
            "verify" => crate::release_steps::run_schema_verify(
                arguments
                    .get_one::<String>("engine")
                    .context("--engine is required")?,
                arguments
                    .get_one::<String>("migrations")
                    .map_or("migrations", String::as_str),
                arguments
                    .get_one::<String>("project-dir")
                    .map_or(".", String::as_str),
            ),
            other => anyhow::bail!("unknown schema release operation {other}"),
        },
        "python" => crate::release_steps::run_python(
            arguments
                .get_one::<String>("operation")
                .context("Python release operation is missing")?,
            &arguments,
        ),
        "swift" => crate::native::run(arguments, &runtime),
        "surface" => crate::surface::run(&arguments, &runtime),
        "documentation" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("documentation operation is missing")?;
            crate::documentation::run(&action, arguments, &runtime)
        }
        "changelog" => crate::changelog::run(
            Path::new(
                arguments
                    .get_one::<String>("root")
                    .context("changelog root is missing")?,
            ),
            arguments
                .get_one::<String>("version")
                .context("changelog version is missing")?,
        ),
        _ => unreachable!("Clap admitted an undeclared product command"),
    }
}

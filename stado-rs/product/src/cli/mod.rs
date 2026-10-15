mod catalog;
mod compiles;
pub use compiles::compiles;
mod lifecycle;
mod native;
pub(crate) mod registry;
mod swift;
use crate::common::Runtime;
use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command};
pub(crate) use native::cargo_operations;
use std::path::PathBuf;

fn value(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help).action(ArgAction::Set)
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
        .arg(
            value(
                "workspace",
                "The workspace of canonical checkouts this operation reads and builds from; \
                 default WISENT_WORKSPACE, else ~/Documents/CodingProjects/Wisent",
            )
            .global(true),
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(catalog::command())
        .subcommand(
            Command::new("dependencies")
                .about(
                    "Read from each product's source every call it makes to another catalog \
                     product (program, resolver, hosted API, state directory, environment) and \
                     refuse a product the catalog does not show running without it: a dependency \
                     undeclared, declared with no alternative (bound; by design when the catalog \
                     says why in bound_because), or with no alternative_source in the workspace",
                )
                .arg(positional("PRODUCT").help("One product; every catalog product when omitted"))
                .arg(flag("json", "Print each product's repositories, dependencies and the lines that reach them as JSON")),
        )
        .subcommand(registry::command())
        .subcommand(registry::creation())
        .subcommand(lifecycle::installation(
            "install",
            "Install one product surface from its canonical recipe or exact qualified release",
            lifecycle::Shape::BuildsWithPlan,
        ))
        .subcommand(lifecycle::installation(
            "update",
            "Update one product surface while retaining exact rollback bytes",
            lifecycle::Shape::BuildsWithPlan,
        ))
        .subcommand(lifecycle::installation(
            "finish",
            "Finish, with this binary, an installation whose build an install retained after \
             its quality gates and build passed: sign, stage, place and activate it without \
             building again. An installation of the running Stado hands its built Stado this \
             step; a revision with no retained build is refused",
            lifecycle::Shape::Builds,
        ))
        .subcommands(lifecycle::changes())
        .subcommand(lifecycle::locks())
        .subcommand(lifecycle::sync())
        .subcommand(lifecycle::signing())
        .subcommand(
            Command::new("paths")
                .about("Inspect actual executable ownership: PATH collisions and executables in ~/.stado/bin and ~/.local/bin no installed product owns")
                .arg(flag("json", "Print observed path collisions and unowned executables")),
        )
        .subcommand(native::cargo_command())
        .subcommand(native::compiler_cache())
        .subcommand(native::build_trees())
        .subcommand(native::node_runtime())
        .subcommand(native::source_bundle())
        .subcommand(native::python())
        .subcommand(native::npm())
        .subcommand(native::schema())
        .subcommand(native::deliver())
        .subcommand(
            Command::new("linkage")
                .about("Prove each .app bundle's @rpath dependencies resolve inside it, as dyld resolves them")
                .arg(clap::Arg::new("bundle").required(true).action(ArgAction::Append).help("A .app bundle; repeatable")),
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
        .subcommand(swift::command())
        .subcommand(native::editor())
        .subcommand(native::surface())
        .subcommand(native::documentation())
        .subcommand(crate::changelog::command::command())
}

/// Run one parsed `stado product` invocation as `build`, returning its exit status.
pub fn run(mut matches: clap::ArgMatches, build: crate::Build) -> Result<i32> {
    let _ = crate::BUILD.set(build);
    if let Some(workspace) = matches.get_one::<String>("workspace") {
        crate::common::use_workspace(PathBuf::from(workspace))?;
    }
    let runtime = Runtime::new(matches.get_one::<String>("catalog").map(PathBuf::from))?;
    let (action, mut arguments) = matches
        .remove_subcommand()
        .context("product command is missing")?;
    match action.as_str() {
        "catalog" => crate::catalog::run(arguments, &runtime),
        "dependencies" => crate::dependencies::run(arguments, &runtime),
        "create" => crate::creation::run(arguments, &runtime),
        "registry" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("registry operation is missing")?;
            crate::registry::run(&action, arguments, &runtime)
        }
        // Each lifecycle verb is the installation module's; one arm per verb
        // so the router names exactly the subcommands registered above.
        "install" => crate::install::run(&action, arguments, &runtime),
        "update" => crate::install::run(&action, arguments, &runtime),
        "finish" => crate::install::run(&action, arguments, &runtime),
        "status" => crate::install::run(&action, arguments, &runtime),
        "cancel" => crate::install::run(&action, arguments, &runtime),
        "remove" => crate::install::run(&action, arguments, &runtime),
        "rollback" => crate::install::run(&action, arguments, &runtime),
        "sync" => crate::install::run(&action, arguments, &runtime),
        "locks" => lifecycle::report_locks(&runtime, arguments.get_flag("json")),
        "signing" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("signing operation is missing")?;
            crate::signing::run(&action, arguments, &runtime)
        }
        "paths" => crate::paths::run(arguments, &runtime),
        "cargo" => crate::cargo::run(arguments, &runtime),
        "compiler-cache" => crate::compiler_cache::run(
            arguments
                .get_one::<String>("operation")
                .context("compiler-cache operation is missing")?,
            arguments.get_flag("json"),
            &runtime.home,
        ),
        "build-trees" => crate::build_trees::run(
            arguments
                .get_one::<String>("operation")
                .context("build-trees operation is missing")?,
            arguments.get_flag("json"),
            &runtime,
        ),
        "node-runtime" => crate::node_runtime::run(
            arguments
                .get_one::<String>("operation")
                .context("node-runtime operation is missing")?,
            arguments.get_flag("json"),
            &runtime.home,
        ),
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
                    .context("--migrations is missing")?,
                arguments
                    .get_one::<String>("project-dir")
                    .context("--project-dir is missing")?,
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
        "editor" => crate::native::serve_editor(arguments, &runtime),
        "surface" => crate::surface::run(&arguments, &runtime),
        "documentation" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("documentation operation is missing")?;
            crate::documentation::run(&action, arguments, &runtime)
        }
        "changelog" => crate::changelog::command::run(&arguments),
        _ => unreachable!("Clap admitted an undeclared product command"),
    }
}

mod builders;
use builders::{catalog, lifecycle, name};
pub(crate) use builders::registry;
mod native;
mod route;
pub use route::{compiled_product, compiles, run};
mod swift;
use clap::{Arg, ArgAction, Command};
pub(crate) use native::cargo_operations;

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
        .arg(
            Arg::new("purpose")
                .long("purpose")
                .value_name("DEFECT")
                .help(
                    "An open Oko defect this operation serves, named to Oko's admission \
                     (`oko admit build --purpose DEFECT`) before anything compiles, so a consent \
                     the operator limited to that defect admits it; repeat for each defect",
                )
                .action(ArgAction::Append)
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
        .subcommand(crate::advisories::command())
        .subcommand(registry::command())
        .subcommand(name::command())
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
        .subcommand(
            Command::new("restart")
                .about("Restart a local macOS application bundle after its installed bytes change")
                .arg(Arg::new("bundle").value_name("APP.app").required(true))
                .arg(flag("if-running", "Leave a stopped application stopped"))
                .arg(flag("json", "Print the observed LaunchServices result")),
        )
        .subcommand(lifecycle::locks())
        .subcommand(lifecycle::sync())
        .subcommand(lifecycle::policy())
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

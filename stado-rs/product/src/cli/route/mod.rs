//! Running one parsed `stado product` invocation: each subcommand the
//! builder in `cli` declares routed to the module that owns it, and the
//! check of which invocations need the product compiled.

mod compiles;
mod locks;
pub use compiles::{compiled_product, compiles};

use crate::common::Runtime;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Run one parsed `stado product` invocation as `build`, returning its exit status.
pub fn run(mut matches: clap::ArgMatches, build: crate::Build) -> Result<i32> {
    crate::register_build(build);
    if let Some(workspace) = matches.get_one::<String>("workspace") {
        crate::common::use_workspace(PathBuf::from(workspace))?;
    }
    let (action, mut arguments) = matches
        .remove_subcommand()
        .context("product command is missing")?;
    if action == "restart" {
        return crate::desktop_app::from_arguments(&arguments);
    }
    let runtime = Runtime::new(matches.get_one::<String>("catalog").map(PathBuf::from))?;
    match action.as_str() {
        "catalog" => crate::catalog::run(arguments, &runtime),
        "advisories" => crate::advisories::run(arguments, &runtime),
        "dependencies" => crate::dependencies::run(arguments, &runtime),
        "create" => crate::creation::run(arguments, &runtime),
        "registry" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("registry operation is missing")?;
            crate::registry::run(&action, arguments, &runtime)
        }
        "name" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("name operation is missing")?;
            crate::name::run(&action, arguments, &runtime)
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
        "locks" => locks::report_locks(&runtime, arguments.get_flag("json")),
        "policy" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("policy operation is missing")?;
            crate::install::policy::run(&action, arguments, &runtime)
        }
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
        "swift" => match arguments.get_one::<String>("operation").map(String::as_str) {
            Some("api") => crate::surface::run(&arguments, &runtime),
            _ => crate::native::run(arguments, &runtime),
        },
        "editor" => crate::native::serve_editor(arguments, &runtime),
        "surface" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("surface operation is missing")?;
            match action.as_str() {
                "add" => crate::creation::surface::run(arguments, &runtime),
                other => anyhow::bail!("unknown surface operation {other}"),
            }
        }
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

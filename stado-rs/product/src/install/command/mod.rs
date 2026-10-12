//! `stado product install|update|finish|status|remove|rollback|sync`: the
//! arguments of one lifecycle verb, the catalog it reads, and the operation
//! it runs.

use anyhow::{bail, Result};

use super::{handoff, perform, status, sweep, transaction, Coordinate};
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
};

impl<'a> Coordinate<'a> {
    /// The source commit the coordinate binds.
    pub fn revision(self) -> &'a str {
        match self {
            Self::Release { revision, .. } | Self::SourceCommit(revision) => revision,
        }
    }
}

pub fn run(action: &str, arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    if action == "sync" {
        return sweep::run(arguments, runtime);
    }
    let args = Arguments::from_matches(arguments);
    let [id] = args.positional.as_slice() else {
        bail!("{action} requires exactly one product");
    };
    let mut runtime = runtime.clone();
    runtime.wait_for_writer = args.has("--wait");
    runtime.signing = crate::signing::Choice::from_arguments(&args)?;
    runtime.finishing = handoff::finishing(action, &args.positional);
    let runtime = &runtime;
    // clap's `--surface` parser (cli::lifecycle::surface) refused any other.
    let surface = args.required("--surface")?;
    let host = args.optional("--host")?;
    // clap declares --release-version and --source-commit only on the verbs
    // that build (cli::lifecycle), so no other verb can carry them here.
    let coordinate = match (
        args.optional("--release-version")?,
        args.optional("--source-commit")?,
    ) {
        (None, None) => None,
        (Some(version), Some(revision)) => Some(Coordinate::Release { version, revision }),
        (None, Some(commit)) => Some(Coordinate::SourceCommit(commit)),
        (Some(_), None) => bail!("--release-version needs the --source-commit bound to it"),
    };
    let document = catalog::current(runtime)?;
    let product = catalog::product(&document, id)?;
    let without: Vec<String> = args
        .optional("--without")?
        .into_iter()
        .flat_map(|list| list.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    for name in &without {
        catalog::product(&document, name)?;
    }
    if args.has("--check-arguments") {
        emit(&serde_json::json!({
            "action": action,
            "product": id,
            "surface": surface,
            "arguments": "accepted",
        }))?;
        return Ok(libc::EXIT_SUCCESS);
    }
    // The verbs that build execute their recipe, so they read the catalog
    // from a commit (catalog::committed), never the checkout's working tree.
    let build = |runtime: &Runtime| -> Result<serde_json::Value> {
        let mut building = runtime.clone();
        building.create_checkouts = true;
        let (document, read) = catalog::committed::for_installation(
            runtime,
            id,
            coordinate.map(Coordinate::revision),
        )?;
        building.catalog_read = Some(read);
        let product = catalog::product(&document, id)?;
        Ok(serde_json::to_value(perform(
            &building,
            &document,
            product,
            surface,
            host,
            coordinate,
            &without,
            &mut Vec::new(),
        )?)?)
    };
    let report = match action {
        "status" => status::inspect(runtime, product, surface, host)?,
        "install" => build(runtime)?,
        "update" => build(runtime)?,
        "finish" => build(runtime)?,
        "remove" => serde_json::to_value(transaction::lifecycle::remove(
            runtime, product, surface, host,
        )?)?,
        "rollback" => {
            serde_json::to_value(transaction::lifecycle::rollback(runtime, product, surface)?)?
        }
        _ => bail!("unknown installation operation {action}"),
    };
    emit(&report)?;
    Ok(libc::EXIT_SUCCESS)
}

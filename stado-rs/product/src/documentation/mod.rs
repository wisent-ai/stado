mod github;
mod index;
mod markdown;
mod pages;
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
};
use anyhow::{bail, Result};

/// The GitHub organization Wisent's repositories live in, which the markdown policy
/// reads unless `--org` names another.
const WISENT_ORGANIZATION: &str = "wisent-ai";

pub fn run(operation: &str, arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let arguments = Arguments::from_matches(arguments);
    let report = match operation {
        "cli-pages" => {
            if !arguments.positional.is_empty() {
                bail!("documentation cli-pages does not take positional arguments");
            }
            pages::report(
                &catalog::cli_catalog(&catalog::current(runtime)?)?,
                arguments.many("--origin"),
            )?
        }
        "markdown-policy" => {
            if !arguments.positional.is_empty() {
                bail!("documentation markdown-policy does not take positional arguments");
            }
            let organization = match arguments.optional("--org")? {
                Some(organization) => organization,
                None => WISENT_ORGANIZATION,
            };
            markdown::report(organization, arguments.has("--include-archived"))?
        }
        "index" => {
            if !arguments.positional.is_empty() {
                bail!("documentation index does not take positional arguments");
            }
            index::report(arguments.optional("--root")?, arguments.has("--check"))?
        }
        other => bail!("unknown documentation operation {other}"),
    };
    emit(&report)?;
    Ok(i32::from(report["ok"] != true))
}

mod github;
mod markdown;
mod pages;
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
};
use anyhow::{bail, Result};

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
            markdown::report(
                arguments.optional("--org")?.unwrap_or("wisent-ai"),
                arguments.has("--include-archived"),
            )?
        }
        other => bail!("unknown documentation operation {other}"),
    };
    emit(&report)?;
    Ok(i32::from(report["ok"] != true))
}

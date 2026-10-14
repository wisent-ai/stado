//! Which `stado product` invocations hold the disk janitor's cleanup lock,
//! read through the real command tree Stado parses: an installation, an
//! update, a Cargo build and the compiling Swift operations hold it; the
//! editor's language server (`editor serve`) and an operation that builds
//! nothing (`catalog`) do not, so an open editor never keeps the janitor from
//! its turn.

use anyhow::{ensure, Context, Result};
use clap::Command;

/// The `stado product` command as Stado builds it, parsing `words`.
fn parsed(words: &[&str]) -> Result<clap::ArgMatches> {
    let command = stado_product::cli::augment(Command::new("product").no_binary_name(true));
    command
        .try_get_matches_from(words.iter().copied())
        .with_context(|| format!("stado product {} does not parse", words.join(" ")))
}

fn holds(words: &[&str], expected: bool) -> Result<()> {
    let compiles = stado_product::cli::compiles(&parsed(words)?);
    ensure!(
        compiles == expected,
        "stado product {} {} the cleanup lock, but should {}",
        words.join(" "),
        if compiles { "holds" } else { "does not hold" },
        if expected { "hold it" } else { "not hold it" }
    );
    Ok(())
}

fn main() -> Result<()> {
    holds(&["editor", "serve"], false)?;
    holds(&["swift", "build"], true)?;
    holds(&["swift", "index"], true)?;
    holds(&["catalog"], false)?;
    holds(&["cargo", "results"], false)?;
    println!("product compiles: editor serve holds nothing; swift build and index hold the cleanup lock; catalog holds nothing");
    Ok(())
}

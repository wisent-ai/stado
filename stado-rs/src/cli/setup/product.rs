//! `stado product`: the canonical Wisent product catalog and every product
//! lifecycle operation — catalog, registry, creation, installation, signing,
//! scheduled updates, and canonical Cargo and Swift source builds.
//!
//! The catalog is `catalog/products.yml` at the root of this repository and
//! the implementation is the `stado-product` crate beside this one. Both were
//! the separate `wisent-products` program until 2026-09-25; Stado already owns
//! the hosts, services and releases those operations act on, so they run here
//! in the same process rather than through a second executable Stado had to
//! download before it could install anything.

use clap::{ArgMatches, Command, FromArgMatches, Subcommand};

use crate::cli::CmdError;

/// The Skarbiec item holding the fleet's Apple certificate and key. It signs
/// every product installation unless the operator names another credential
/// through `WISENT_CODESIGN_CREDENTIAL_ITEM` or `WISENT_CODESIGN_CERTIFICATE_PEM`.
///
/// It is named because the machine running Stado keeps no identity of its own:
/// on 2026-09-20 `stado product update skrzynka --surface cli` refused with
/// "Apple signing identity is missing or ambiguous: Apple Development: Created
/// via API (685D4U2G83)" on a Mac whose keychain held one unrelated
/// certificate, while that exact certificate was in the vault the whole time.
pub const SIGNING_CREDENTIAL_ITEM: &str = "desktop-signing-apple-development";

/// One parsed `stado product` invocation. Its operations are declared once, by
/// the product crate, and parsed by the same clap tree as the rest of Stado.
#[derive(Debug, Clone)]
pub struct ProductCommands {
    matches: ArgMatches,
}

impl FromArgMatches for ProductCommands {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            matches: matches.clone(),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.matches = matches.clone();
        Ok(())
    }
}

impl Subcommand for ProductCommands {
    fn augment_subcommands(command: Command) -> Command {
        stado_product::cli::augment(command)
    }

    fn augment_subcommands_for_update(command: Command) -> Command {
        stado_product::cli::augment(command)
    }

    fn has_subcommand(name: &str) -> bool {
        stado_product::cli::augment(Command::new("product"))
            .find_subcommand(name)
            .is_some()
    }
}

/// The build every product record names.
pub fn build() -> stado_product::Build {
    stado_product::Build {
        version: env!("CARGO_PKG_VERSION"),
        source_revision: env!("STADO_SOURCE_REVISION"),
        signing_item: SIGNING_CREDENTIAL_ITEM,
    }
}

pub async fn dispatch(command: ProductCommands) -> Result<(), CmdError> {
    // Product operations run compilers, codesign and `stado` subcommands and
    // wait for them; they are blocking work, kept off the async workers.
    let status =
        tokio::task::spawn_blocking(move || stado_product::cli::run(command.matches, build()))
            .await
            .map_err(|error| CmdError::click(format!("product operation stopped: {error}")))?
            .map_err(|error| CmdError::click(format!("{error:#}")))?;
    if status == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(status))
    }
}

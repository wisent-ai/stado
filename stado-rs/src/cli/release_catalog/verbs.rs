//! `stado release catalog publisher declare|withdraw` and `stado release
//! catalog input pin`: the release catalog's nouns that take a verb.

use clap::Subcommand;

use super::publisher;
use crate::cli::CmdError;

#[derive(Subcommand)]
pub(super) enum PublisherCommands {
    /// Declare one product's release publisher across the fleet: mint its
    /// item on the vault owner, grant the release client access, declare it on
    /// every participating host, then reconcile each host's verifier grant.
    /// `build submit` and `release submit` run this themselves for a product
    /// this host has not declared, with this host as the client and the vault
    /// owner read from `skarbiec bond status`; run it by hand only to declare
    /// further API targets or reloads.
    Declare {
        /// The product, as its release manifest names it.
        product: String,
        /// The registry host whose vault is authoritative.
        #[arg(long)]
        owner: String,
        /// The registry host that runs `release submit`.
        #[arg(long)]
        client: String,
        /// Further hosts that serve the release API; repeat for several.
        #[arg(long = "target")]
        targets: Vec<String>,
        /// HOST=SERVICE: the host Stado process on HOST, whose release API
        /// caches the publisher table for its lifetime, reconciled after the
        /// declaration lands on that host; repeat for several hosts. Any
        /// other unit, a retired one included, is refused.
        #[arg(long = "reload")]
        reloads: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a retired product's release publisher declaration from the
    /// vault owner, this host and every --target, then reconcile each host's
    /// release verifier. Refuses while the release catalog still holds the
    /// product. The vault item stays, so a returning product re-declares.
    Withdraw {
        /// The product, as its release manifest named it.
        product: String,
        /// Further hosts that serve the release API; repeat for several.
        #[arg(long = "target")]
        targets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(super) enum InputCommands {
    /// Make one committed revision of another repository, or only the paths of
    /// it the build reads (--path, repeatable), an immutable build input of the
    /// product in CHECKOUT: archive it, store it create-only under the
    /// product's sources, and write `inputs.<name>` of its
    /// `.wisent-release.json`. The build reads it from WISENT_INPUT_<NAME>_DIR.
    /// --cargo publishes Cargo.lock's private crates, --git-bundle the commit's
    /// history, --swiftpm a Swift package's committed resolution.
    Pin(publisher::PinInputArgs),
}

pub(super) async fn publisher(command: PublisherCommands) -> Result<(), CmdError> {
    match command {
        PublisherCommands::Declare {
            product,
            owner,
            client,
            targets,
            reloads,
            json,
        } => {
            publisher::declare_publisher(&product, &owner, &client, &targets, &reloads, json).await
        }
        PublisherCommands::Withdraw {
            product,
            targets,
            json,
        } => publisher::withdraw_publisher(&product, &targets, json).await,
    }
}

pub(super) async fn input(command: InputCommands) -> Result<(), CmdError> {
    match command {
        InputCommands::Pin(args) => publisher::pin_input(args).await,
    }
}

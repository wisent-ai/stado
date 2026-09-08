//! `stado service directory` — the fleet's answer to "where is X, and who may
//! use it".
//!
//! The canonical registry grew a `service_directory` block that no source in
//! this tree modelled. It survived only because the registry write paths are
//! lossless: `push` uploads the operator's exact bytes and
//! `push_document_if` serializes a raw document. Nothing could read it, so
//! every client that needed a service address reconstructed one from a host
//! name and a guess about forwarded ports — which is wrong on every machine
//! that is not the one running the service.
//!
//! The block's shape, as the live document carries it:
//!
//! ```json
//! "service_directory": {
//!   "authority":  {"target": "...", "command": "..."},
//!   "generation":
//!     1,
//!   "services": {
//!     "brama": {
//!       "placement_profile": "brama-skarbiec",
//!       "active_host": "control-host",
//!       "endpoints": {"control-host": {"url": "http://127.0.0.1:8080"},
//!                     "operator-host":    {"url": "http://127.0.0.1:8080"}},
//!       "consumers": {"operator": {"capabilities": ["model-routing"]}}
//!     }
//!   }
//! }
//! ```
//!
//! `endpoints` is keyed by the machine ASKING, not by the machine serving.
//! These services bind loopback on their own host, so "where is Brama" has a
//! different true answer per client and the directory states each one instead
//! of leaving every caller to derive it.
//!
//! Everything here reads and mutates the RAW document through
//! `registry::fetch_document` and `registry::commit_document`. There is
//! deliberately no typed model of the block: a model is exactly what deletes
//! the keys it does not know, and this file exists because that already
//! happened to this document.

use clap::Subcommand;

use crate::cli::CmdError;

mod document;
mod report;
mod routes;

pub(crate) use crate::cli::directory::routes::service_port;

use crate::cli::directory::report::publish::publish;
use crate::cli::directory::report::{profiles, show};
use crate::cli::directory::routes::connect::connect;
use crate::cli::directory::routes::consumers::{consumer_add, consumer_rm};
use crate::cli::directory::routes::endpoints::{bind, endpoint};

#[derive(Subcommand)]
pub enum DirectoryCommands {
    /// Print the whole service directory.
    Show {
        #[arg(long)]
        json: bool,
    },

    /// The placement profiles the registry declares.
    ///
    /// A profile is what says a service is SUPPOSED to run somewhere, which is
    /// a different fact from the directory's `active_host` and from whether
    /// anything is listening. Reading it settles an argument this fleet has
    /// already had: `brama-skarbiec` declares units on two hosts, so a Brama
    /// missing from one of them is an unstarted unit rather than a service
    /// that lives elsewhere.
    Profiles {
        #[arg(long)]
        json: bool,
    },

    /// The serving parameters for the host this service is placed on.
    ///
    /// The other side of `connect`: a caller asks how to reach the service,
    /// and the placed host asks how it should serve. Both answers come from
    /// the same placement and the same host records, so moving a service needs
    /// no edit on either side. Refused on a host the service is not placed on,
    /// because a gateway that binds where nothing placed it is the thing every
    /// caller then has to be protected from.
    Bind {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Answer for this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// A usable route to one service, derived from where it is placed.
    ///
    /// `endpoint` reports what the directory was told; this works out what is
    /// true. The service is placed on exactly one host, so the address is a
    /// function of that placement and of who is asking: loopback when the
    /// asker is the placed host, and the placed host's routable address
    /// otherwise. Nothing per-caller is stored, so moving the service moves
    /// every caller with it.
    ///
    /// There is no other address. If the service is placed somewhere that does not
    /// answer, that is what this says -- resolving to something local instead
    /// is how a caller ends up talking to a process nobody placed.
    Connect {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        /// Which declared consumer is calling. Selects among this machine's
        /// resolver adapters when a service is bound once per consumer.
        #[arg(long)]
        consumer: Option<String>,
        /// Report the address without proving anything answers there.
        #[arg(long)]
        no_verify: bool,
        #[arg(long)]
        json: bool,
    },

    /// The address this machine should use for one service.
    ///
    /// Resolves against the asking target rather than the active host,
    /// because a loopback-bound service has a different address on every
    /// client. A target with no entry is reported as exactly that.
    Endpoint {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Write this machine's forward markers from the directory.
    ///
    /// Several products resolve a service's address from an owner-only file
    /// under `~/.stado/forwards/<service>.local` rather than an environment
    /// variable - Skarbiec's credential bridge reads `skarbiec.local`, and
    /// `weles-admission.local` the same way. Nothing wrote those files. They
    /// were produced by hand, which is why one on this fleet named a port no
    /// service has ever bound while the directory held the right answer for
    /// the same host all along.
    ///
    /// The address is per-caller, so this resolves for the asking target and
    /// writes only what the directory declares. A service with no endpoint
    /// for this machine is reported and skipped, never guessed.
    ///
    /// Markers the directory does not declare are reported as `fossil` on
    /// every run and removed only under `--prune`. Nothing has ever removed
    /// one: operator-host carries 11 markers of which the directory declares
    /// 3, including three different names for one endpoint and two different
    /// ports for one relationship, and a consumer holding an old name resolves
    /// it forever.
    Publish {
        /// Publish one service instead of every declared endpoint.
        #[arg(long)]
        service: Option<String>,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        /// Delete the markers the directory does not declare. Without this
        /// they are only reported: a marker is an address something on this
        /// host dials, and removing one by default makes a publish a reaper.
        #[arg(long)]
        prune: bool,
        #[arg(long)]
        json: bool,
    },

    /// Declare that a consumer may use a service.
    ConsumerAdd {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to declare.
        consumer: String,
        /// Capability to grant; repeat for several.
        #[arg(long = "capability")]
        capabilities: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    /// Remove a consumer's declaration.
    ConsumerRm {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to remove.
        consumer: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: DirectoryCommands) -> Result<(), CmdError> {
    match command {
        DirectoryCommands::Show { json } => show(json).await,
        DirectoryCommands::Publish {
            service,
            target,
            prune,
            json,
        } => publish(service, target, prune, json).await,
        DirectoryCommands::Profiles { json } => profiles(json).await,
        DirectoryCommands::Bind { name, target, json } => bind(&name, target, json).await,
        DirectoryCommands::Connect {
            name,
            target,
            consumer,
            no_verify,
            json,
        } => connect(&name, target, consumer, no_verify, json).await,
        DirectoryCommands::Endpoint { name, target, json } => endpoint(&name, target, json).await,
        DirectoryCommands::ConsumerAdd {
            name,
            consumer,
            capabilities,
            json,
        } => consumer_add(&name, &consumer, capabilities, json).await,
        DirectoryCommands::ConsumerRm {
            name,
            consumer,
            json,
        } => consumer_rm(&name, &consumer, json).await,
    }
}

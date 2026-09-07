//! `stado web`'s command surface.
//!
//! Nothing here decides anything: every variant's behaviour lives in the
//! module `super::super::dispatch` sends it to.

use clap::Subcommand;

use crate::cli::web::{edge, origin};

// `Declare` carries every flag the three kinds of declaration between them
// need, so it is much larger than `List` or `Quality`. Boxing a clap
// subcommand variant would put an indirection in the parser's own type for
// nothing: this enum is constructed once per process. `ReleaseCommands`
// carries the same allow for the same reason.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum WebCommands {
    /// Declare a web product: where it runs, as whom, and on which hostname.
    ///
    /// With `--redirect-to` the product is a hostname and a target and
    /// nothing else: the edge answers it with a 308 and there is no unit, no
    /// release and no host, so `--host`, `--port` and `--consumer` are
    /// refused beside it.
    Declare {
        /// Product name, matching `product` in its `.wisent-release.json`.
        name: String,
        /// Registry target the unit runs on.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        host: Option<String>,
        /// Loopback port the unit listens on.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        port: Option<u16>,
        /// Public hostname the product answers on.
        #[arg(long)]
        hostname: String,
        /// Skarbiec consumer identity the unit runs as.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        consumer: Option<String>,
        /// Where this hostname redirects, instead of running a unit:
        /// an https URL with a host and no query or fragment.
        #[arg(long = "redirect-to", conflicts_with_all = ["upstream_service", "path_prefix"])]
        redirect_to: Option<String>,
        /// A path prefix under a hostname another declaration owns, for a unit
        /// product mounted inside that hostname's site block.
        #[arg(long = "path-prefix", conflicts_with_all = ["redirect_to", "upstream_service"])]
        path_prefix: Option<String>,
        /// A registry service this hostname is published in front of, instead
        /// of a unit this product owns. The service directory answers which
        /// host it is active on and which address it serves.
        #[arg(long = "upstream-service")]
        upstream_service: Option<String>,
        /// Request path that proves the unit is ready.
        #[arg(long, default_value = "/", conflicts_with_all = ["redirect_to", "upstream_service"])]
        readyz: String,
        /// Which edge terminates TLS for the hostname.
        #[arg(
            long,
            default_value = "stado",
            value_parser = clap::builder::PossibleValuesParser::new(crate::config::WEB_API_EDGES)
        )]
        edge: String,
        /// Plain environment entry, `NAME=value`; repeatable.
        #[arg(long = "env", conflicts_with_all = ["redirect_to", "upstream_service"])]
        env: Vec<String>,
        /// Secret environment entry, `NAME=item#field`; repeatable.
        #[arg(long = "secret", conflicts_with_all = ["redirect_to", "upstream_service"])]
        secrets: Vec<String>,
        /// Declared database the product reads.
        #[arg(long, conflicts_with_all = ["redirect_to", "upstream_service"])]
        database: Option<String>,
        /// Field of the database's Skarbiec item to deliver.
        #[arg(long, default_value = "pooler_url", conflicts_with_all = ["redirect_to", "upstream_service"])]
        database_field: String,
        /// Variable the database field is delivered as.
        #[arg(long, default_value = "DATABASE_URL", conflicts_with_all = ["redirect_to", "upstream_service"])]
        database_variable: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// List declared web products with their host, port, hostname and unit.
    List {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a web product: stop and forget its unit, drop its DNS record.
    Remove {
        /// Product name.
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Install the published release as a managed unit and deliver its
    /// environment, then verify that it answers.
    Deploy {
        /// Product name.
        name: String,
        /// Exact published version; defaults to the newest stable release.
        #[arg(long)]
        version: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Report each product's declared edge, live unit/port state, and observed DNS.
    ///
    /// A missing or invalid selected Stado edge is reported in edge_error,
    /// never accepted as an external edge with unknown addresses. Any
    /// non-serving product makes the command exit with status 1.
    Status {
        /// Product name; omit for every declared product.
        name: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Publish a declared hostname: edge, then DNS, then wait for the certificate.
    Route {
        /// Product name.
        name: String,
        /// Report what would change and exit non-zero, without writing.
        #[arg(long)]
        check: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// The public edge: the fleet host that holds an address and terminates
    /// TLS for every `stado`-edge hostname.
    #[command(subcommand)]
    Edge(edge::EdgeCommands),
    /// Public origins: the hostnames the internet reaches a Stado surface
    /// through, and whether anything outside this network can resolve them.
    #[command(subcommand)]
    Origin(origin::OriginCommands),
    /// Install the locked dependency tree and run the product's own checks.
    ///
    /// Runs on a release worker, inside the checkout Stado prepared. The
    /// recipe in `.wisent-release.json` names this command; an operator does
    /// not run it by hand.
    Quality {
        /// The directory the site is served from, repository-relative.
        ///
        /// Only a static site names one. Absent, the site root is the
        /// checkout root.
        #[arg(long)]
        root: Option<String>,
    },
    /// Build the checked-out web product and stage its runnable tarball.
    ///
    /// Runs on a release worker, inside the checkout Stado prepared.
    Build {
        /// The directory the site is served from, repository-relative.
        ///
        /// Only a static site names one — for a product whose build script
        /// writes into `dist/`, or whose site is not at the repository root.
        /// Absent, the site root is the checkout root.
        #[arg(long)]
        root: Option<String>,
    },
}

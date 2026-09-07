//! `stado web` — hosting a Node web product on the fleet.
//!
//! One product is one declaration: which release artifact it runs, on which
//! host and port, under which Skarbiec consumer, with which environment, and
//! behind which public hostname. Everything else in this module is that
//! declaration being acted on.
//!
//! The build half runs on a release worker (`stado web quality`,
//! `stado web build`), so a product's `.wisent-release.json` names one Stado
//! command instead of carrying a build script of its own — thirty-four web
//! products do not need thirty-four of those.
//!
//! The run half is the service registry, unchanged: `stado web deploy` renders
//! the declaration into the same `ServiceDeclaration` any other unit uses and
//! installs it with `stado service deploy`, mints the unit's consumer grant
//! with `stado service grant-sync`, and delivers every secret with
//! `stado service secret-sync` — one field of one item into one variable, over
//! the host channel. A database credential is resolved for the unit's own
//! consumer through `stado database resolve`, so a product that is not a
//! declared consumer of a database cannot receive its credential.
//!
//! The publish half is `stado web route`: the hostname is reconciled into the
//! edge proxy's configuration, then its DNS record moves to the edge, then the
//! hostname is polled until it answers over TLS. That order is forced rather
//! than chosen — Let's Encrypt delivers its challenge to whatever the name
//! resolves to, so the certificate cannot exist until after the record moves,
//! and the site block has to exist before it so the first request after the
//! cutover finds a proxy that knows the name.
//!
//! `stado web origin` is the boundary below all of that: the hostnames the
//! public internet reaches a Stado surface through, whether anything outside
//! this network can resolve them, and what publishes them. A product hostname
//! and a public origin are separate declarations because they fail separately
//! — on 2026-09-07 `brama.wisent.com` answered 502 `DNS_HOSTNAME_NOT_FOUND`
//! at its edge while the release origin answered 503 `dns_unresolved`, and
//! neither had a declaration anything could refuse or report.

mod build;
mod deploy;
mod edge;
mod origin;
mod plane;
mod route;
mod status;

use super::CmdError;

pub(crate) use plane::{declare, list, mutate_web, product, remove, DeclareRequest, WebCommands};

/// Every managed web unit is labelled under one domain, so `launchctl list`
/// and `stado service list` both group them without a naming convention
/// anyone has to remember.
pub(crate) const UNIT_DOMAIN: &str = "com.wisent.web";

/// Where a web product's released bytes are installed on its host. The
/// release machinery already owns `$HOME/.stado/services/<name>/current`.
pub(crate) fn unit_label(product: &str) -> String {
    format!("{UNIT_DOMAIN}.{product}")
}

/// The launcher the staged tarball carries, relative to the install root.
pub(crate) const LAUNCHER: &str = "bin/start-web";

pub(crate) async fn dispatch(command: WebCommands) -> Result<(), CmdError> {
    match command {
        WebCommands::Declare {
            name,
            host,
            port,
            hostname,
            consumer,
            redirect_to,
            upstream_service,
            path_prefix,
            readyz,
            edge,
            env,
            secrets,
            database,
            database_field,
            database_variable,
            json,
        } => declare(DeclareRequest {
            name: &name,
            // clap guarantees the three unit arguments are present unless
            // this is a redirect, so the defaults here are only ever reached
            // by a redirect, which has no unit to describe.
            host: host.as_deref().unwrap_or(""),
            port: port.unwrap_or(0),
            hostname: &hostname,
            consumer: consumer.as_deref().unwrap_or(""),
            redirect_to: redirect_to.as_deref(),
            upstream_service: upstream_service.as_deref(),
            path_prefix: path_prefix.as_deref(),
            readyz: &readyz,
            edge: &edge,
            env: &env,
            secrets: &secrets,
            database: database.as_deref(),
            database_field: &database_field,
            database_variable: &database_variable,
            json,
        }),
        WebCommands::List { json } => list(json),
        WebCommands::Remove { name, json } => remove(&name, json).await,
        WebCommands::Deploy {
            name,
            version,
            json,
        } => deploy::deploy(&name, version.as_deref(), json).await,
        WebCommands::Status { name, json } => status::status(name.as_deref(), json).await,
        WebCommands::Route { name, check, json } => route::route(&name, check, json).await,
        WebCommands::Edge(command) => edge::dispatch(command).await,
        WebCommands::Origin(command) => origin::dispatch(command).await,
        WebCommands::Quality { root } => build::quality(root.as_deref()),
        WebCommands::Build { root } => build::build(root.as_deref()),
    }
}

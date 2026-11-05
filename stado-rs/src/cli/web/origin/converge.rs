//! `stado web origin converge` — make the declared target publish the
//! declared paths, then read back what actually happened.
//!
//! Three things are read back, in the order that decides the verdict. The
//! node's own handler table, because a zero exit status from the verb that
//! writes it is a claim and not evidence. A public resolver, because a
//! publication is not an origin: a target can have funnel on and the release
//! route proxied while the tailnet's own authoritative nameserver answers
//! NXDOMAIN for the name, with nothing in the product saying so. And the
//! declared origin itself over the public internet, because the
//! only proof that a public origin serves is a public request that it answers.
//!
//! The refusal that remains after a successful convergence is therefore not a
//! Stado failure and does not pretend to be repairable here: a `*.ts.net`
//! name is published in public DNS by Tailscale's own control plane, not by
//! the node, and no command in this product can create that record.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::public_origin::{self, funnel, PublicOrigin, ResolutionState, WEB_EDGE};

pub(crate) async fn converge(
    name: &str,
    apply: bool,
    github_variables: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let variables = parse_variables(github_variables, apply)?;
    let document = crate::cli::registry::fetch_document().await?;
    let origin = public_origin::declaration(&document, name).ok_or_else(|| {
        CmdError::usage(format!(
            "no public origin {name:?} is declared; declare it with `stado web origin declare`"
        ))
    })?;
    // An origin declared before the catalog said which services may be
    // public is held to the same rule before it is published again.
    super::declare::refuse_unless_public(&origin).await?;
    let converged = if origin.publication == WEB_EDGE {
        converge_web_edge(&origin, apply, json_output).await
    } else {
        converge_funnel(&origin, apply, json_output).await
    };
    converged?;
    publish_variables(&origin, &variables).await
}

/// Each `--github-variable REPOSITORY/NAME`, refused when malformed or given
/// without --apply (a plan writes nothing).
fn parse_variables(given: &[String], apply: bool) -> Result<Vec<(String, String)>, CmdError> {
    if !given.is_empty() && !apply {
        return Err(CmdError::usage(
            "--github-variable writes the origin once it serves, which only --apply converges; \
             add --apply, or leave the variable out to see the plan",
        ));
    }
    given
        .iter()
        .map(|entry| match entry.split_once('/') {
            Some((repository, name))
                if !repository.is_empty()
                    && !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
            {
                Ok((repository.to_string(), name.to_string()))
            }
            _ => Err(CmdError::usage(format!(
                "--github-variable {entry:?} must be <repository>/<NAME>, a repository of the \
                 organization and a variable name of letters, digits and underscores"
            ))),
        })
        .collect()
}

/// Write the origin that now serves to each named Actions variable.
async fn publish_variables(
    origin: &PublicOrigin,
    variables: &[(String, String)],
) -> Result<(), CmdError> {
    if variables.is_empty() {
        return Ok(());
    }
    let token = crate::deploy::host_precheck_runner::github_credential()
        .await
        .map_err(CmdError::from)?;
    for (repository, name) in variables {
        crate::deploy::host_precheck_runner::repository_name(repository).map_err(CmdError::from)?;
        crate::deploy::host_precheck_runner::set_repository_variable(
            repository,
            name,
            &origin.origin(),
            &token,
        )
        .map_err(CmdError::from)?;
        println!(
            "{repository} vars.{name} = {} (the origin {} serves)",
            origin.origin(),
            origin.name
        );
    }
    Ok(())
}

/// The Funnel publication: converge the node's handler table, then read back
/// what actually happened.
async fn converge_funnel(
    origin: &PublicOrigin,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let target = crate::cli::canonical_host(&origin.target).await?;
    let (publication, changes) = funnel::converge(origin, &target, &runner, apply)
        .await
        .map_err(CmdError::from)?;
    let resolution = public_origin::resolve(&origin.hostname).await;
    let readback = read_back(origin, resolution.state).await;

    let refusal = refusal_for(origin, &publication, &resolution, &readback);
    let status = if refusal.is_some() {
        "refused"
    } else if changes.iter().any(|change| change.change == "added") {
        "converged"
    } else {
        "unchanged"
    };
    let receipt = json!({
        "schema": "stado.public-origin-converge-receipt.v1",
        "name": origin.name,
        "target": origin.target,
        "publication": origin.publication,
        "origin": origin.origin(),
        "applied": apply,
        "status": status,
        "handlers": changes.iter().map(funnel::HandlerChange::to_json).collect::<Vec<_>>(),
        "funnel": {
            "enabled": publication.funnel_enabled,
            "port": funnel::FUNNEL_PORT,
            "published_paths": publication.published,
            "missing_paths": publication.missing,
            "undeclared_paths": publication.undeclared,
        },
        "resolution": resolution.to_json(),
        "readback": readback.to_json(),
        "refusal": refusal,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "{} {} on {} ({})",
            status, origin.name, origin.target, origin.publication
        );
        for change in &changes {
            println!("  {} {} -> {}", change.change, change.path, change.upstream);
        }
        println!("  resolution: {}", resolution.detail);
        println!("  readback:   {}", readback.detail);
        if let Some(refusal) = &refusal {
            println!("  refused:    {refusal}");
        }
    }
    if refusal.is_some() {
        return Err(CmdError::silent(1));
    }
    Ok(())
}

/// Converge a `web-edge` origin: publish the web declaration that owns its
/// hostname with `stado web route` (which prints its own receipt), then ask
/// the world whether the name resolves and the first declared path answers.
async fn converge_web_edge(
    origin: &PublicOrigin,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let owner = super::web_edge_owner(&origin.hostname).map_err(CmdError::declaration)?;
    let routed = crate::cli::web::route::route(&owner.product, !apply, json_output).await;
    let resolution = public_origin::resolve(&origin.hostname).await;
    let readback = read_back(origin, resolution.state).await;
    let refusal = match &routed {
        Err(error) => Some(format!(
            "stado web route {} did not publish {}: {error}",
            owner.product, origin.hostname
        )),
        Ok(()) => match resolution.state {
            ResolutionState::Unresolved => Some(format!(
                "{} is routed by web declaration {} on the {} edge, and no public resolver has \
                 an A or AAAA record for it yet: {}",
                origin.hostname, owner.product, owner.edge, resolution.detail
            )),
            ResolutionState::Unavailable => Some(format!(
                "whether {} is publicly resolvable could not be established: {}",
                origin.hostname, resolution.detail
            )),
            ResolutionState::Resolved if readback.state != "answered" => {
                Some(format!("{}: {}", origin.origin(), readback.detail))
            }
            ResolutionState::Resolved => None,
        },
    };
    let status = match (&refusal, apply) {
        (Some(_), _) => "refused",
        (None, true) => "converged",
        (None, false) => "planned",
    };
    let receipt = json!({
        "schema": "stado.public-origin-converge-receipt.v1",
        "name": origin.name,
        "target": origin.target,
        "publication": origin.publication,
        "origin": origin.origin(),
        "applied": apply,
        "status": status,
        "web_declaration": {
            "product": owner.product,
            "edge": owner.edge,
            "upstream_service": owner.upstream_service,
        },
        "resolution": resolution.to_json(),
        "readback": readback.to_json(),
        "refusal": refusal,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "{} {} through web declaration {} on the {} edge",
            status, origin.name, owner.product, owner.edge
        );
        println!("  resolution: {}", resolution.detail);
        println!("  readback:   {}", readback.detail);
        if let Some(refusal) = &refusal {
            println!("  refused:    {refusal}");
        }
    }
    if refusal.is_some() {
        return Err(CmdError::silent(1));
    }
    Ok(())
}

/// One public request against the declared origin's first declared path.
///
/// Any HTTP answer proves the publication carries that path: the release route
/// answers 400 to a request naming no `uri`, and a 400 from the declared path
/// is a live handler, while a 404 from the same path is a table that does not
/// carry it. What matters is that a public client got an answer at all.
struct ReadBack {
    state: &'static str,
    status: Option<u16>,
    detail: String,
}

impl ReadBack {
    fn to_json(&self) -> Value {
        json!({
            "state": self.state,
            "status": self.status,
            "detail": self.detail,
        })
    }
}

async fn read_back(origin: &PublicOrigin, resolution: ResolutionState) -> ReadBack {
    if resolution != ResolutionState::Resolved {
        return ReadBack {
            state: "not-attempted",
            status: None,
            detail: format!(
                "no public request was made: {} has no public address record to send it to",
                origin.hostname
            ),
        };
    }
    let path = match origin.paths.first() {
        Some(path) => path,
        None => {
            return ReadBack {
                state: "not-attempted",
                status: None,
                detail: "the declaration names no path to read".to_string(),
            }
        }
    };
    let url = format!("{}{path}", origin.origin());
    let client = match crate::cli::storage::fleet_https_client() {
        Ok(client) => client,
        Err(error) => {
            return ReadBack {
                state: "unreachable",
                status: None,
                detail: format!("this Stado could not build its HTTPS client: {error}"),
            }
        }
    };
    match crate::wait::request(client.get(&url)).await {
        Ok(response) => {
            let status = response.status().as_u16();
            ReadBack {
                state: "answered",
                status: Some(status),
                detail: format!("{url} answered HTTP {status} to a public request"),
            }
        }
        Err(error) => ReadBack {
            state: "unreachable",
            status: None,
            detail: format!("{url} did not answer a public request: {error}"),
        },
    }
}

fn refusal_for(
    origin: &PublicOrigin,
    publication: &funnel::Publication,
    resolution: &public_origin::Resolution,
    readback: &ReadBack,
) -> Option<String> {
    if !publication.missing.is_empty() {
        return Some(format!(
            "{} does not publish {} as declared; funnel_enabled={}",
            origin.target,
            publication.missing.join(", "),
            publication.funnel_enabled
        ));
    }
    match resolution.state {
        // The node's own table publishes the name and grants it Funnel, and the
        // public resolvers answer NXDOMAIN for it. Those two facts together are
        // not a Stado condition at all: a `ts.net` name is published by the
        // tailnet, so the node has the grant locally and the tailnet is not
        // serving it. This state makes every release object read from outside
        // answer 503 while every Stado read here says "published". Say which
        // side is missing, or an
        // operator re-runs converge against a host that is already doing its
        // half.
        ResolutionState::Unresolved if publication.funnel_enabled => Some(format!(
            "{host} publishes {name} with funnel enabled, and the public resolvers answer no A \
             or AAAA record for it. Both halves cannot be true here: a ts.net name is published \
             by the tailnet, not by this fleet, so the node holds its half and the tailnet is \
             not serving the name. Nothing in Stado repairs that — the tailnet policy has to \
             grant this node Funnel, or the declaration has to name the origin the tailnet does \
             serve (`stado web origin declare`). Detail: {detail}",
            host = origin.target,
            name = origin.hostname,
            detail = resolution.detail
        )),
        ResolutionState::Unresolved => Some(format!(
            "the declared origin {} is published by {}'s {} but has no public A or AAAA record, \
             so no public edge can fetch it: {}",
            origin.hostname, origin.target, origin.publication, resolution.detail
        )),
        ResolutionState::Unavailable => Some(format!(
            "whether {} is publicly resolvable could not be established: {}",
            origin.hostname, resolution.detail
        )),
        ResolutionState::Resolved if readback.state != "answered" => {
            Some(format!("{}: {}", origin.origin(), readback.detail))
        }
        ResolutionState::Resolved => None,
    }
}

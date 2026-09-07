//! `stado web origin converge` — make the declared target publish the
//! declared paths, then read back what actually happened.
//!
//! Three things are read back, in the order that decides the verdict. The
//! node's own handler table, because a zero exit status from the verb that
//! writes it is a claim and not evidence. A public resolver, because a
//! publication is not an origin: on 2026-09-07 this exact target had funnel
//! on and `/api/release/object` proxied while `ts.net`'s own authoritative
//! nameserver answered NXDOMAIN for the name, and nothing in the product said
//! so. And the declared origin itself over the public internet, because the
//! only proof that a public origin serves is a public request that it answers.
//!
//! The refusal that remains after a successful convergence is therefore not a
//! Stado failure and does not pretend to be repairable here: a `*.ts.net`
//! name is published in public DNS by Tailscale's own control plane, not by
//! the node, and no command in this product can create that record.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::public_origin::{self, funnel, PublicOrigin, ResolutionState};

pub(crate) async fn converge(name: &str, apply: bool, json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let origin = public_origin::declaration(&document, name).ok_or_else(|| {
        CmdError::usage(format!(
            "no public origin {name:?} is declared; declare it with `stado web origin declare`"
        ))
    })?;
    let runner = crate::deploy::production_runner();
    let target = crate::deploy::host_channel::canonical_target(&origin.target)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "the registry could not resolve target {}: {}",
                origin.target, error.0
            ))
        })?;
    let (publication, changes) = funnel::converge(&origin, &target, &runner, apply)
        .await
        .map_err(|error| CmdError::click(error.0))?;
    let resolution = public_origin::resolve(&origin.hostname).await;
    let readback = read_back(&origin, resolution.state).await;

    let refusal = refusal_for(&origin, &publication, &resolution, &readback);
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
    match client.get(&url).send().await {
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

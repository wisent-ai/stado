//! `stado web origin declare` and `stado web origin remove`.
//!
//! The declare path asks the world before it writes. A registry validator
//! cannot: every reader runs it, on every fetch, and a resolver query there
//! would put the network on the read path of the fleet's survival authority.
//! So shape is judged offline by `public_origin::validate_registry_contract`
//! and PUBLICNESS is judged here, once, by the command that creates the claim.
//!
//! It refuses `dns_unresolved` and it also refuses `dns_unavailable`, for
//! opposite reasons. A name with no record is not a public origin. A resolver
//! that could not be asked has established nothing, and a declaration nobody
//! checked is the defect this capability exists to remove — so the command
//! says which of the two happened and neither is silently accepted.
//!
//! One name is exempt from the first refusal: a `tailscale-funnel` node's own
//! tailnet name. Tailscale publishes that record only while the node's Funnel
//! is on, and Funnel is switched on by `converge` for a declared origin, so
//! demanding the record first made the funnel origin impossible to declare
//! at all — which is how charless-mac-mini had no release origin from
//! 2026-09-22. It is declared as `pending-publication`, and `converge` and
//! `status` judge the public name afterwards.

use serde_json::{json, Value};

use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::public_origin::{self, PublicOrigin, POLICY_KEY};

pub(crate) struct DeclareRequest<'a> {
    pub name: &'a str,
    pub hostname: &'a str,
    pub target: &'a str,
    pub publication: &'a str,
    pub upstream: &'a str,
    pub paths: &'a [String],
    pub json: bool,
}

pub(crate) async fn declare(request: DeclareRequest<'_>) -> Result<(), CmdError> {
    let origin = PublicOrigin {
        name: request.name.to_string(),
        hostname: request.hostname.to_string(),
        target: request.target.to_string(),
        publication: request.publication.to_string(),
        upstream: request.upstream.to_string(),
        paths: request.paths.to_vec(),
    };
    // A `web-edge` origin is published by the web declaration that owns its
    // hostname; declaring the origin first would record a public claim no
    // edge carries.
    if origin.publication == public_origin::WEB_EDGE {
        super::web_edge_owner(&origin.hostname).map_err(|detail| {
            CmdError::click(format!(
                "refusing to declare public origin {:?}: {detail}",
                origin.name
            ))
            .stating(FailureCode::Refused)
        })?;
    }
    let resolution = public_origin::resolve(&origin.hostname).await;
    match resolution.state {
        public_origin::ResolutionState::Unresolved if !publishes_own_tailnet_name(&origin) => {
            // The refusal sentence already says what the resolver found, so
            // the resolution detail is not appended: one condition, one
            // sentence, and an operator reading it twice starts looking for
            // the second cause.
            return Err(CmdError::click(public_origin::unresolvable_refusal(
                &origin.name,
                &origin.hostname,
            ))
            .stating(FailureCode::Refused));
        }
        public_origin::ResolutionState::Unresolved => {}
        public_origin::ResolutionState::Unavailable => {
            return Err(CmdError::click(format!(
                "refusing to declare public origin {:?}: whether {} has a public A or AAAA \
                 record could not be established, and a public origin nobody checked is the \
                 declaration this command exists to prevent — {}",
                origin.name, origin.hostname, resolution.detail
            ))
            .stating(FailureCode::Refused))
        }
        public_origin::ResolutionState::Resolved => {}
    }
    let publicness = if resolution.state == public_origin::ResolutionState::Resolved {
        "public"
    } else {
        PENDING_PUBLICATION
    };

    let row = public_origin::to_row(&origin);
    let name = origin.name.clone();
    let change = std::cell::Cell::new("declared");
    let generation = commit_document(|document| {
        let mut next = document.clone();
        let object = next
            .as_object_mut()
            .ok_or_else(|| CmdError::click("the registry document must be an object"))?;
        let rows = object
            .entry(POLICY_KEY.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let rows = rows
            .as_array_mut()
            .ok_or_else(|| CmdError::click(format!("registry.{POLICY_KEY} must be an array")))?;
        match rows
            .iter()
            .position(|existing| existing["name"].as_str() == Some(name.as_str()))
        {
            Some(index) => {
                change.set("replaced");
                rows[index] = row.clone();
            }
            None => {
                change.set("declared");
                rows.push(row.clone());
            }
        }
        Ok(next)
    })
    .await?;

    let report = json!({
        "schema": "stado.public-origin-declaration-receipt.v1",
        "name": origin.name,
        "hostname": origin.hostname,
        "origin": origin.origin(),
        "target": origin.target,
        "publication": origin.publication,
        "upstream": origin.upstream,
        "paths": origin.paths,
        "change": change.get(),
        "generation": generation,
        "resolution": resolution.to_json(),
        "publicness": publicness,
    });
    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} public origin {} -> {} on {} ({}); generation {generation}",
            change.get(),
            origin.name,
            origin.origin(),
            origin.target,
            origin.publication,
        );
        println!("  {}", resolution.detail);
        if publicness == PENDING_PUBLICATION {
            println!(
                "  {} is the node's own tailnet name: Tailscale publishes it while Funnel is on; \
                 `stado web origin converge {} --apply` turns Funnel on and reads the public name back",
                origin.hostname, origin.name
            );
        }
    }
    Ok(())
}

/// The declaration was accepted before its name is public, because only the
/// publication it declares can make the name public.
const PENDING_PUBLICATION: &str = "pending-publication";

/// Whether this origin is a `tailscale-funnel` publication of the declared
/// target's own tailnet name. The registry validator already refuses a
/// tailnet name whose node label is not the target.
fn publishes_own_tailnet_name(origin: &PublicOrigin) -> bool {
    origin.publication == public_origin::TAILSCALE_FUNNEL
        && origin
            .hostname
            .strip_suffix(".ts.net")
            .and_then(|node| node.split('.').next())
            == Some(origin.target.as_str())
}

pub(crate) async fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let declared = public_origin::declaration(&document, name)
        .ok_or_else(|| CmdError::usage(format!("no public origin {name:?} is declared")))?;
    let wanted = name.to_string();
    let generation = commit_document(move |document| {
        let mut next = document.clone();
        let object = next
            .as_object_mut()
            .ok_or_else(|| CmdError::click("the registry document must be an object"))?;
        if let Some(Value::Array(rows)) = object.get_mut(POLICY_KEY) {
            rows.retain(|row| row["name"].as_str() != Some(wanted.as_str()));
            // An empty array is a section nothing validates against and a
            // reader would carry forever. A fleet that publishes nothing
            // publicly declares no key at all.
            if rows.is_empty() {
                object.remove(POLICY_KEY);
            }
        }
        Ok(next)
    })
    .await?;
    let report = json!({
        "schema": "stado.public-origin-declaration-receipt.v1",
        "name": declared.name,
        "hostname": declared.hostname,
        "origin": declared.origin(),
        "target": declared.target,
        "change": "removed",
        "generation": generation,
        "publication": declared.publication,
        "note": "the target's publication is unchanged: a handler table is shared by every product on that hostname",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "removed public origin {} ({}); generation {generation}",
            declared.name,
            declared.origin()
        );
        println!("  the target's publication is unchanged; withdraw it with its own operation if that is what you meant");
    }
    Ok(())
}

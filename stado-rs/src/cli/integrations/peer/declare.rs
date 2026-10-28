//! `stado market peer add|edit|remove|list|show`: the peer fleets this
//! fleet trades with.

use serde_json::{json, Value};

use crate::cli::{CmdError, PeerCommands};
use crate::providers::peer::{declaration, loop_role, offer, PeerDeclaration};

use super::{cmd, our_store};

/// Answer one `stado market peer` command.
pub(crate) async fn declare(command: &PeerCommands) -> Result<(), CmdError> {
    let store = our_store().await?;
    match command {
        PeerCommands::Add {
            name,
            origin,
            namespace,
            credential_role,
            ca_file,
            json,
        } => {
            if declaration::load(&store, name)
                .await
                .map_err(cmd)?
                .is_some()
            {
                return Err(CmdError::refused(format!(
                    "peer {name} is already declared; `stado market peer edit {name}` changes it"
                )));
            }
            let now = chrono::Utc::now().to_rfc3339();
            let stored = declaration::save(
                &store,
                &PeerDeclaration {
                    name: name.clone(),
                    origin: origin.trim().to_string(),
                    namespace: namespace.trim().to_string(),
                    credential_role: credential_role.trim().to_string(),
                    ca_file: ca_file.clone(),
                    declared_at: now.clone(),
                    changed_at: now,
                },
            )
            .await
            .map_err(cmd)?;
            print_peer("declared", &stored, &[], *json)
        }
        PeerCommands::Edit {
            name,
            origin,
            namespace,
            credential_role,
            ca_file,
            clear_ca_file,
            json,
        } => {
            let mut peer = declaration::require(&store, name).await.map_err(cmd)?;
            if origin.is_none()
                && namespace.is_none()
                && credential_role.is_none()
                && ca_file.is_none()
                && !clear_ca_file
            {
                return Err(CmdError::usage(format!(
                    "stado market peer edit {name} names nothing to change: give --origin, \
                     --namespace, --credential-role, --ca-file or --clear-ca-file"
                )));
            }
            if let Some(origin) = origin {
                peer.origin = origin.trim().to_string();
            }
            if let Some(namespace) = namespace {
                peer.namespace = namespace.trim().to_string();
            }
            if let Some(role) = credential_role {
                peer.credential_role = role.trim().to_string();
            }
            if let Some(ca_file) = ca_file {
                peer.ca_file = Some(ca_file.clone());
            }
            if *clear_ca_file {
                peer.ca_file = None;
            }
            peer.changed_at = chrono::Utc::now().to_rfc3339();
            let stored = declaration::save(&store, &peer).await.map_err(cmd)?;
            print_peer("changed", &stored, &[], *json)
        }
        PeerCommands::Remove { name, json } => {
            declaration::require(&store, name).await.map_err(cmd)?;
            let home = crate::config_file::home().map_err(CmdError::declaration)?;
            if let Some(holder) = crate::deploy::role_lock::holder(&home, &loop_role(name))? {
                return Err(CmdError::refused(format!(
                    "peer {name} is held by the follow loop pid {} on this host since {}; \
                     stop that loop first, and its lending worker drains before it ends",
                    holder.pid, holder.since
                )));
            }
            offer::remove_for_peer(&store, name).await.map_err(cmd)?;
            declaration::remove(&store, name).await.map_err(cmd)?;
            let answer = json!({"peer": name, "removed": true});
            if *json {
                crate::cli::print_answer(&answer, true)?;
            } else {
                println!("peer {name} retired, with every host's offer to it");
            }
            Ok(())
        }
        PeerCommands::List { json } => {
            let peers = declaration::list(&store).await.map_err(cmd)?;
            if *json {
                crate::cli::print_answer(&serde_json::to_value(&peers)?, true)?;
                return Ok(());
            }
            if peers.is_empty() {
                println!("no peer fleet is declared; `stado market peer add` declares one");
            }
            for peer in &peers {
                println!(
                    "{}  {}  namespace {}  role {}",
                    peer.name, peer.origin, peer.namespace, peer.credential_role
                );
            }
            Ok(())
        }
        PeerCommands::Show { name, json } => {
            let peer = declaration::require(&store, name).await.map_err(cmd)?;
            let offers = offer::for_peer(&store, name).await.map_err(cmd)?;
            let offers: Vec<Value> = offers
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()?;
            print_peer("declared", &peer, &offers, *json)
        }
    }
}

fn print_peer(
    what: &str,
    peer: &PeerDeclaration,
    offers: &[Value],
    json: bool,
) -> Result<(), CmdError> {
    if json {
        let mut answer = serde_json::to_value(peer)?;
        answer["offers"] = Value::from(offers.to_vec());
        crate::cli::print_answer(&answer, true)?;
        return Ok(());
    }
    println!("peer {} {what}", peer.name);
    println!("origin:     {}", peer.origin);
    println!("namespace:  {}", peer.namespace);
    println!(
        "credential: the item playing {}, field token",
        peer.credential_role
    );
    match &peer.ca_file {
        Some(ca_file) => println!("authority:  {ca_file}"),
        None => println!("authority:  the system roots"),
    }
    println!("changed:    {}", peer.changed_at);
    for offer in offers {
        println!("offer:      {offer}");
    }
    Ok(())
}

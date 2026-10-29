//! A node's standing attachment to a relay: the one connection a host no
//! peer can dial keeps open, over which the relay asks it to dial back when
//! a peer wants it. Each dial-back is then served like any connection that
//! arrived at this node's own listener.

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream};

use super::Context;
use crate::network::{dial, key, peers, Answer, Request};

/// Keep this node attached to `relay` for as long as the registry declares
/// it a relay, attaching again after the period its declaration names
/// whenever an attachment ends.
pub(super) async fn keep(context: Arc<Context>, relay: String) {
    loop {
        let ended = attached(&context, &relay).await;
        let retry = match peers::copy()
            .await
            .and_then(|registry| peers::node_of(&registry, &relay).map(|(_, node)| node.relay))
        {
            Ok(Some(declared)) => declared.attach_retry_seconds,
            Ok(None) => {
                eprintln!("stado network: {relay} is no longer a relay; not attaching to it again ({ended})");
                return;
            }
            Err(cause) => {
                eprintln!("stado network: attachment to relay {relay} ended ({ended}) and its declaration cannot be read: {cause}");
                return;
            }
        };
        eprintln!("stado network: attachment to relay {relay} ended ({ended}); attaching again in the {retry}s its declaration names");
        tokio::time::sleep(std::time::Duration::from_secs(retry)).await;
    }
}

/// One attachment, until it ends; the sentence says why it ended.
async fn attached(context: &Arc<Context>, relay: &str) -> String {
    let link = match session(context, relay, Request::Attach).await {
        Ok(link) => link,
        Err(cause) => return format!("could not attach: {cause}"),
    };
    eprintln!("stado network: {} attached to relay {relay}", context.host);
    let mut lines = BufReader::new(link).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<Answer>(&line) {
                Ok(Answer::Dial { token }) => {
                    let (context, relay) = (Arc::clone(context), relay.to_string());
                    tokio::spawn(async move {
                        let outcome =
                            match session(&context, &relay, Request::Accept { token }).await {
                                Ok(carried) => super::handle(carried, Arc::clone(&context)).await,
                                Err(cause) => Err(format!("could not dial back: {cause}")),
                            };
                        if let Err(cause) = outcome {
                            eprintln!("stado network: connection relayed by {relay}: {cause}");
                        }
                    });
                }
                Ok(other) => return format!("the relay sent {other:?} where a dial-back belongs"),
                Err(error) => return format!("the relay sent an unreadable line: {error}"),
            },
            Ok(None) => return "the relay closed it".to_string(),
            Err(error) => return format!("reading it failed: {error}"),
        }
    }
}

/// A session with `relay`'s node asking `request`, through whichever of its
/// endpoints answers first.
async fn session(context: &Context, relay: &str, request: Request) -> Result<DuplexStream, String> {
    let (_, node) = peers::find(relay).await?;
    let relay_key = key::decode_public(&node.public_key)?;
    let attempts = node
        .endpoints
        .iter()
        .map(|endpoint| {
            let what = format!("endpoint {endpoint}");
            let (endpoint, keypair, relay_key, request) = (
                endpoint.clone(),
                context.keypair.clone(),
                relay_key.clone(),
                request.clone(),
            );
            let attempt: BoxFuture<'static, Result<DuplexStream, String>> = Box::pin(async move {
                let tcp = dial::tcp(&endpoint).await?;
                dial::open(tcp, &keypair, &relay_key, &request)
                    .await
                    .map(|(stream, _)| stream)
            });
            (what, attempt)
        })
        .collect::<Vec<_>>();
    if attempts.is_empty() {
        return Err(format!("relay {relay} declares no endpoint"));
    }
    dial::first(attempts)
        .await
        .map_err(|failures| failures.join("; "))
}

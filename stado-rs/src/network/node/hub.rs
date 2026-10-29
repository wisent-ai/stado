//! A relay node's side: hosts attached to it, the dial-backs it asked them
//! for, and the connections it carries. The relay decrypts only its own
//! session with each side; what it copies between them is the end-to-end
//! session of the two hosts.

use std::collections::HashMap;
use std::sync::Mutex;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};

use crate::network::secure::{Session, Stream};
use crate::network::{dial, peers, Answer};

/// Attached hosts and dial-backs in flight.
#[derive(Default)]
pub(crate) struct Hub {
    attached: Mutex<HashMap<String, (u64, mpsc::UnboundedSender<String>)>>,
    pending: Mutex<HashMap<String, oneshot::Sender<DuplexStream>>>,
    attachments: std::sync::atomic::AtomicU64,
}

fn poisoned<T>(_: T) -> String {
    "the relay's attachment table was poisoned by a panicked connection".to_string()
}

impl Hub {
    /// Carry `from`'s connection to `host`'s node: through its attachment
    /// when it holds one, otherwise to the endpoints it declares.
    pub(crate) async fn carry<S: Stream>(
        &self,
        mut session: Session<S>,
        host: &str,
        from: &str,
        relay: &str,
    ) -> Result<(), String> {
        let reached = match self.reach(host).await {
            Ok(reached) => reached,
            Err(reason) => {
                let reason = format!("relay {relay} could not reach {host}: {reason}");
                session
                    .send(&Answer::Refused {
                        reason: reason.clone(),
                    })
                    .await?;
                return Err(format!("{from}: {reason}"));
            }
        };
        session
            .send(&Answer::Ok {
                host: host.to_string(),
            })
            .await?;
        let plain = session.into_plain();
        match reached {
            Reached::Attached(stream) => super::splice(plain, stream).await,
            Reached::Direct(stream) => super::splice(plain, stream).await,
        }
    }

    async fn reach(&self, host: &str) -> Result<Reached, String> {
        let sender = self
            .attached
            .lock()
            .map_err(poisoned)?
            .get(host)
            .map(|(_, sender)| sender.clone());
        if let Some(sender) = sender {
            let token = uuid::Uuid::new_v4().to_string();
            let (deliver, delivered) = oneshot::channel();
            self.pending
                .lock()
                .map_err(poisoned)?
                .insert(token.clone(), deliver);
            if sender.send(token.clone()).is_ok() {
                let waiting = crate::wait::begin(
                    crate::wait::Kind::Network,
                    "attached host's dial-back",
                    host,
                );
                match delivered.await {
                    Ok(stream) => {
                        waiting.done();
                        return Ok(Reached::Attached(stream));
                    }
                    Err(_) => waiting.failed("its attachment ended before it dialled back"),
                }
            }
            self.pending.lock().map_err(poisoned)?.remove(&token);
        }
        let (_, node) = peers::find(host).await?;
        let attempts = node
            .endpoints
            .iter()
            .map(|endpoint| {
                let what = endpoint.clone();
                let endpoint = endpoint.clone();
                let attempt: BoxFuture<'static, Result<tokio::net::TcpStream, String>> =
                    Box::pin(async move { dial::tcp(&endpoint).await });
                (what, attempt)
            })
            .collect::<Vec<_>>();
        if attempts.is_empty() {
            return Err(format!(
                "{host} is not attached here and declares no endpoint"
            ));
        }
        dial::first(attempts)
            .await
            .map(Reached::Direct)
            .map_err(|failures| {
                format!(
                    "{host} is not attached here and no endpoint answered ({})",
                    failures.join("; ")
                )
            })
    }

    /// Keep `from`'s standing attachment until it ends, sending it each
    /// dial-back a peer's connection needs.
    pub(crate) async fn attach<S: Stream>(
        &self,
        mut session: Session<S>,
        from: &str,
        relay: &str,
    ) -> Result<(), String> {
        let (sender, mut tokens) = mpsc::unbounded_channel();
        let generation = self
            .attachments
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.attached
            .lock()
            .map_err(poisoned)?
            .insert(from.to_string(), (generation, sender));
        session
            .send(&Answer::Ok {
                host: relay.to_string(),
            })
            .await?;
        let (mut reader, mut writer) = tokio::io::split(session.into_plain());
        let mut ignored = [0_u8; 1];
        let ended = loop {
            tokio::select! {
                token = tokens.recv() => {
                    let Some(token) = token else { break "replaced by a newer attachment".to_string() };
                    let mut line = serde_json::to_vec(&Answer::Dial { token }).map_err(|error| error.to_string())?;
                    line.push(b'\n');
                    if let Err(error) = writer.write_all(&line).await {
                        break format!("the dial-back request could not be sent: {error}");
                    }
                }
                read = reader.read(&mut ignored) => match read {
                    Ok(0) | Err(_) => break "the host closed it".to_string(),
                    Ok(_) => {}
                },
            }
        };
        let mut attached = self.attached.lock().map_err(poisoned)?;
        if attached
            .get(from)
            .is_some_and(|(current, _)| *current == generation)
        {
            attached.remove(from);
        }
        Err(format!("{from}'s attachment ended: {ended}"))
    }

    /// Hand a dial-back to the connection waiting for it.
    pub(crate) async fn accept<S: Stream>(
        &self,
        mut session: Session<S>,
        token: &str,
        relay: &str,
    ) -> Result<(), String> {
        let Some(deliver) = self.pending.lock().map_err(poisoned)?.remove(token) else {
            let reason = "no connection on this relay waits for that dial-back".to_string();
            session
                .send(&Answer::Refused {
                    reason: reason.clone(),
                })
                .await?;
            return Err(reason);
        };
        session
            .send(&Answer::Ok {
                host: relay.to_string(),
            })
            .await?;
        deliver
            .send(session.into_plain())
            .map_err(|_| "the connection that asked for this dial-back ended".to_string())
    }
}

enum Reached {
    Attached(DuplexStream),
    Direct(tokio::net::TcpStream),
}

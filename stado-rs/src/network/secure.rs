//! One authenticated, encrypted Noise session over any byte stream, and the
//! plaintext stream it becomes once the request is answered.
//!
//! Every message is a two-byte big-endian length and a Noise message of at
//! most the 65535 bytes the Noise specification allows (section 3). A
//! message whose plaintext is empty says the sender will write no more, so a
//! half-closed SSH stream closes half of the session, not all of it.

use serde::{de::DeserializeOwned, Serialize};
use snow::StatelessTransportState;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};

use super::key::Keypair;

/// The Noise specification's largest message: "Noise messages ... must not
/// exceed 65535 bytes" (section 3), which `snow` enforces without exporting.
const MAX_MESSAGE: usize = 65535;
/// The authentication tag every ChaChaPoly transport message carries.
const TAG: usize = 16;
/// The largest plaintext one transport message holds.
pub(crate) const MAX_PLAINTEXT: usize = MAX_MESSAGE - TAG;

pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Stream for T {}

/// A session whose handshake has completed.
pub(crate) struct Session<S> {
    stream: S,
    transport: StatelessTransportState,
    sent: u64,
    received: u64,
}

async fn write_message<W: AsyncWrite + Unpin>(
    stream: &mut W,
    message: &[u8],
) -> std::io::Result<()> {
    let length = u16::try_from(message.len())
        .map_err(|_| std::io::Error::other("a Noise message exceeds 65535 bytes"))?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(message).await?;
    stream.flush().await
}

async fn read_message<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut length = [0_u8; 2];
    stream.read_exact(&mut length).await?;
    let mut message = vec![0_u8; usize::from(u16::from_be_bytes(length))];
    stream.read_exact(&mut message).await?;
    Ok(message)
}

fn builder<'a>(keypair: &'a Keypair) -> Result<snow::Builder<'a>, String> {
    let params = super::NOISE_PARAMS
        .parse()
        .map_err(|error| format!("Noise parameters {}: {error}", super::NOISE_PARAMS))?;
    snow::Builder::new(params)
        .local_private_key(&keypair.private)
        .and_then(|builder| builder.prologue(super::PROLOGUE))
        .map_err(|error| format!("prepare the Noise handshake: {error}"))
}

/// Open a session to the node whose static key is `peer`.
pub(crate) async fn initiate<S: Stream>(
    mut stream: S,
    keypair: &Keypair,
    peer: &[u8],
) -> Result<Session<S>, String> {
    let mut handshake = builder(keypair)?
        .remote_public_key(peer)
        .and_then(snow::Builder::build_initiator)
        .map_err(|error| format!("prepare the Noise handshake: {error}"))?;
    let mut buffer = vec![0_u8; MAX_MESSAGE];
    let written = handshake
        .write_message(&[], &mut buffer)
        .map_err(|error| format!("write the Noise handshake: {error}"))?;
    write_message(&mut stream, &buffer[..written])
        .await
        .map_err(|error| format!("send the Noise handshake: {error}"))?;
    let reply = read_message(&mut stream).await.map_err(|error| {
        format!("the node closed the connection instead of answering the handshake: {error}")
    })?;
    handshake
        .read_message(&reply, &mut buffer)
        .map_err(|error| {
            format!("the node's handshake answer does not verify against its registry key: {error}")
        })?;
    finish(stream, handshake)
}

/// The responder's half after the first message: the initiator's key is
/// known and not yet answered, so the caller decides whom it admits.
pub(crate) struct Pending<S> {
    stream: S,
    handshake: snow::HandshakeState,
    pub(crate) remote: Vec<u8>,
}

/// Read an initiator's first handshake message.
pub(crate) async fn accept<S: Stream>(
    mut stream: S,
    keypair: &Keypair,
) -> Result<Pending<S>, String> {
    let mut handshake = builder(keypair)?
        .build_responder()
        .map_err(|error| format!("prepare the Noise handshake: {error}"))?;
    let first = read_message(&mut stream)
        .await
        .map_err(|error| format!("read the Noise handshake: {error}"))?;
    let mut payload = vec![0_u8; MAX_MESSAGE];
    handshake
        .read_message(&first, &mut payload)
        .map_err(|error| format!("the handshake does not verify: {error}"))?;
    let remote = handshake
        .get_remote_static()
        .ok_or("the handshake carries no static key")?
        .to_vec();
    Ok(Pending {
        stream,
        handshake,
        remote,
    })
}

impl<S: Stream> Pending<S> {
    /// Answer the handshake.
    pub(crate) async fn complete(mut self) -> Result<Session<S>, String> {
        let mut buffer = vec![0_u8; MAX_MESSAGE];
        let written = self
            .handshake
            .write_message(&[], &mut buffer)
            .map_err(|error| format!("write the Noise handshake answer: {error}"))?;
        write_message(&mut self.stream, &buffer[..written])
            .await
            .map_err(|error| format!("send the Noise handshake answer: {error}"))?;
        finish(self.stream, self.handshake)
    }
}

fn finish<S>(stream: S, handshake: snow::HandshakeState) -> Result<Session<S>, String> {
    let transport = handshake
        .into_stateless_transport_mode()
        .map_err(|error| format!("the Noise handshake did not finish: {error}"))?;
    Ok(Session {
        stream,
        transport,
        sent: 0,
        received: 0,
    })
}

impl<S: Stream> Session<S> {
    /// Send one JSON value as one transport message.
    pub(crate) async fn send<T: Serialize>(&mut self, value: &T) -> Result<(), String> {
        let text = serde_json::to_vec(value).map_err(|error| format!("encode: {error}"))?;
        let mut buffer = vec![0_u8; text.len() + TAG];
        let written = self
            .transport
            .write_message(self.sent, &text, &mut buffer)
            .map_err(|error| format!("encrypt: {error}"))?;
        self.sent += 1;
        write_message(&mut self.stream, &buffer[..written])
            .await
            .map_err(|error| format!("send: {error}"))
    }

    /// Receive one JSON value.
    pub(crate) async fn receive<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        let message = read_message(&mut self.stream)
            .await
            .map_err(|error| format!("the peer closed the session: {error}"))?;
        let mut plain = vec![0_u8; message.len()];
        let read = self
            .transport
            .read_message(self.received, &message, &mut plain)
            .map_err(|error| format!("a message does not decrypt: {error}"))?;
        self.received += 1;
        serde_json::from_slice(&plain[..read])
            .map_err(|error| format!("the peer sent no request: {error}"))
    }

    /// The session as a plaintext stream, its two directions pumped by two
    /// tasks that end with the connection.
    pub(crate) fn into_plain(self) -> DuplexStream {
        let (ours, theirs) = tokio::io::duplex(MAX_PLAINTEXT);
        let (mut network_read, mut network_write) = tokio::io::split(self.stream);
        let (mut plain_read, mut plain_write) = tokio::io::split(ours);
        let transport = std::sync::Arc::new(self.transport);
        let decrypting = std::sync::Arc::clone(&transport);
        let mut received = self.received;
        tokio::spawn(async move {
            let mut plain = vec![0_u8; MAX_MESSAGE];
            while let Ok(message) = read_message(&mut network_read).await {
                let Ok(read) = decrypting.read_message(received, &message, &mut plain) else {
                    break;
                };
                received += 1;
                if read == 0 || plain_write.write_all(&plain[..read]).await.is_err() {
                    break;
                }
            }
            let _ = plain_write.shutdown().await;
        });
        let mut sent = self.sent;
        tokio::spawn(async move {
            let mut plain = vec![0_u8; MAX_PLAINTEXT];
            let mut message = vec![0_u8; MAX_MESSAGE];
            loop {
                let read = plain_read.read(&mut plain).await.unwrap_or(0);
                let Ok(written) = transport.write_message(sent, &plain[..read], &mut message)
                else {
                    break;
                };
                sent += 1;
                if write_message(&mut network_write, &message[..written])
                    .await
                    .is_err()
                    || read == 0
                {
                    break;
                }
            }
            let _ = network_write.shutdown().await;
        });
        theirs
    }
}

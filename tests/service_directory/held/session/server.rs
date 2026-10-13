//! A real SSH server on a loopback port this machine handed out, run with
//! the same SSH implementation Stado dials with. The first session that
//! carries a forwarded channel stands: it accepts every channel opened on it
//! and never forwards a byte, as the resolver's session to a host did while
//! the host answered a new session at once. Every other session forwards
//! its channels to the service, so a new session carries what the standing
//! one holds. Its host key and the client's key are made by `ssh-keygen`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use russh::server::{run_stream, Auth, ChannelOpenHandle, Config, Handler, Msg, Session};
use russh::Channel;

/// A key pair `ssh-keygen` made at `path`, with no passphrase.
pub(crate) fn key_pair(path: &Path) -> PathBuf {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .expect("ssh-keygen runs");
    assert!(made.success(), "ssh-keygen made {}", path.display());
    path.to_path_buf()
}

struct Carrier {
    /// Whether some session already took the standing role.
    taken: Arc<AtomicBool>,
    /// Whether this session is the one that stands.
    stands: bool,
    service: SocketAddr,
}

impl Handler for Carrier {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _public_key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.stands && !self.taken.swap(true, Ordering::SeqCst) {
            self.stands = true;
        }
        reply.accept().await;
        let stands = self.stands;
        let service = self.service;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            if stands {
                // Held open, answered never: the stream lives as long as
                // this task, which nothing ends.
                let _held = stream;
                return std::future::pending::<()>().await;
            }
            if let Ok(mut upstream) = tokio::net::TcpStream::connect(service).await {
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
            }
        });
        Ok(())
    }
}

/// Serve SSH on `bind` with the host key at `host_key`, forwarding to
/// `service`, for as long as the test runs.
pub(crate) fn serve(bind: &str, host_key: &Path, service: SocketAddr) {
    let listener = std::net::TcpListener::bind(bind).expect("the SSH port binds");
    listener
        .set_nonblocking(true)
        .expect("the SSH listener is non-blocking");
    let key = russh::keys::load_secret_key(host_key, None).expect("the host key loads");
    let config = Arc::new(Config {
        keys: vec![key],
        ..Default::default()
    });
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("a runtime for the SSH server");
        runtime.block_on(async move {
            let listener =
                tokio::net::TcpListener::from_std(listener).expect("the SSH listener is tokio's");
            let taken = Arc::new(AtomicBool::new(false));
            while let Ok((stream, _)) = listener.accept().await {
                let carrier = Carrier {
                    taken: Arc::clone(&taken),
                    stands: false,
                    service,
                };
                let config = Arc::clone(&config);
                tokio::spawn(async move {
                    if let Ok(running) = run_stream(config, stream, carrier).await {
                        let _ = running.await;
                    }
                });
            }
        });
    });
}

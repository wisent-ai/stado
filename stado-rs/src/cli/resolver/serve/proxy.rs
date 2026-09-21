use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::service_resolution::ResolverAdapter;

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;

/// How many consecutive seconds a listener may refuse before it is reported
/// broken rather than pinched.
pub(super) const ACCEPT_FAILURE_LIMIT: u32 = 60;

/// Wait out an accept failure instead of dying of it.
///
/// `accept` answering `EMFILE` says the process is momentarily out of
/// descriptors, which every long-lived server survives by waiting. Returning it
/// killed the resolver instead: launchd counted 166 runs of
/// `com.wisent.stado-resolver` on this workstation, each restart dropping every
/// connection in flight, and four separate `release submit` runs died with
/// `error sending request for url (http://127.0.0.1:18776/...)` in the middle
/// of a publication because of it. One connection's resource error must not
/// close the door for all of them.
///
/// The errno itself is not named, because none of the interesting ones have a
/// stable `ErrorKind` and hard-coding platform numbers is a second thing to be
/// wrong. A failure is waited out and retried; a listener that refuses without
/// pause for [`ACCEPT_FAILURE_LIMIT`] consecutive attempts is the one reported
/// broken, which no descriptor pinch survives and a dead socket always is.
pub(super) async fn accept_backoff(bind: &str, error: &std::io::Error, failures: u32) {
    eprintln!(
        "stado resolver {bind} accept deferred ({failures}/{ACCEPT_FAILURE_LIMIT}), \
         retrying in 1s: {error}"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
}

pub(super) async fn serve_adapter(
    listener: TcpListener,
    adapter: ResolverAdapter,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    let mut failures = 0_u32;
    loop {
        let (client, _) = match listener.accept().await {
            Ok(accepted) => {
                failures = 0;
                accepted
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures >= ACCEPT_FAILURE_LIMIT {
                    return Err(format!(
                        "{} accept failed {failures} times in a row: {error}",
                        adapter.bind
                    ));
                }
                accept_backoff(&adapter.bind, &error, failures).await;
                continue;
            }
        };
        let state = Arc::clone(&state);
        let adapter = adapter.clone();
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(client, &adapter, &state).await {
                eprintln!(
                    "stado resolver adapter service={} consumer={} rejected connection: {}",
                    adapter.service, adapter.consumer, error
                );
            }
        });
    }
}

/// One connection's last activity, in milliseconds since the proxy accepted
/// it, shared by both directions.
///
/// A request/response connection is silent in one direction for as long as
/// the service works, so a per-direction idle timer is not a measure of a
/// dead connection at all: it is a cap on how long an answer may take, and
/// when it fired the proxy shut the half it was copying into. On 2026-09-21
/// that ended four `stado release submit` runs with `connection closed before
/// message completed` against `stado://probierz/...` while the object API on
/// the active host was answering normally and slowly, with 84 jobs running on
/// it. A connection is idle when NEITHER direction has moved a byte inside
/// the window; the retention bound the window exists for is unchanged,
/// because a connection nobody is using is still closed after it.
struct Activity {
    started: std::time::Instant,
    last_millis: std::sync::atomic::AtomicU64,
    /// The client's first bytes were an HTTP request, so a refusal written
    /// back to it is a message it can read rather than noise in a protocol
    /// this proxy knows nothing about.
    client_spoke_http: std::sync::atomic::AtomicBool,
}

impl Activity {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            last_millis: std::sync::atomic::AtomicU64::new(0),
            client_spoke_http: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn touch(&self) {
        self.last_millis.store(
            self.started.elapsed().as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// How long ago either direction last moved a byte.
    fn since(&self) -> Duration {
        let last = self.last_millis.load(std::sync::atomic::Ordering::Relaxed);
        self.started
            .elapsed()
            .saturating_sub(Duration::from_millis(last))
    }

    fn saw_request(&self, head: &[u8]) {
        if http_request_head(head) {
            self.client_spoke_http
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn spoke_http(&self) -> bool {
        self.client_spoke_http
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// What one direction did, so a cut can be reported with the bytes behind it.
struct Transfer {
    bytes: u64,
    cut: bool,
}

async fn copy_until_idle<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle: Duration,
    activity: &Activity,
    on_cut: Option<&[u8]>,
) -> Result<Transfer, std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = match tokio::time::timeout(idle, reader.read(&mut buffer)).await {
            Ok(result) => result?,
            Err(_) => {
                // Silent in this direction only: the other one is moving, so
                // the connection is in use and this is not the socket the
                // window exists to reclaim.
                if activity.since() < idle {
                    continue;
                }
                return cut(writer, bytes, on_cut, activity).await;
            }
        };
        if read == 0 {
            writer.shutdown().await?;
            return Ok(Transfer { bytes, cut: false });
        }
        activity.saw_request(&buffer[..read]);
        activity.touch();
        match tokio::time::timeout(idle, writer.write_all(&buffer[..read])).await {
            Ok(result) => result?,
            Err(_) => {
                if activity.since() < idle {
                    continue;
                }
                return cut(writer, bytes, on_cut, activity).await;
            }
        }
        bytes = bytes.saturating_add(read as u64);
        activity.touch();
    }
}

/// End one direction on the idle window, telling the reader why when it has
/// received nothing yet.
///
/// A client whose connection simply closes reports a transport error — `error
/// sending request ...: connection closed before message completed` — and
/// nothing in that sentence names the proxy, the window or the service that
/// did not answer. Four `stado release submit` runs ended that way on
/// 2026-09-21 and the cause had to be found by reading this file. Once a byte
/// of the answer has already been forwarded the message cannot be retracted,
/// so the close is all that is left and the served log carries the sentence.
async fn cut<W>(
    writer: &mut W,
    bytes: u64,
    on_cut: Option<&[u8]>,
    activity: &Activity,
) -> Result<Transfer, std::io::Error>
where
    W: AsyncWrite + Unpin,
{
    if bytes == 0 && activity.spoke_http() {
        if let Some(message) = on_cut {
            let _ = writer.write_all(message).await;
        }
    }
    writer.shutdown().await?;
    Ok(Transfer { bytes, cut: true })
}

/// Terse refusal for clients that speak HTTP, sent before the prompt close.
const UPSTREAM_REFUSAL: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nupstream unavailable\n";

/// Brief window to sniff the client's first bytes on a refusal where the
/// establishment phase never read any. The connection is already dead, so the
/// only cost of a miss is closing without the 502 body.
const REFUSAL_SNIFF: Duration = Duration::from_millis(250);

/// The stream speaks HTTP when its first bytes open with a request method.
fn http_request_head(head: &[u8]) -> bool {
    const METHODS: [&[u8]; 9] = [
        b"GET ",
        b"POST ",
        b"PUT ",
        b"DELETE ",
        b"HEAD ",
        b"OPTIONS ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    METHODS.iter().any(|method| head.starts_with(method))
}

/// Surface an unreachable upstream instead of letting the client hang: one
/// line naming the service, endpoint, and cause, then an HTTP 502 when the
/// connection's first bytes are an HTTP request, and a prompt close either
/// way. A refused connection is a handled outcome, so callers return `Ok(())`
/// rather than doubling the line through `serve_adapter`.
async fn refuse_connection<R, W>(
    reader: &mut R,
    writer: &mut W,
    adapter: &ResolverAdapter,
    endpoint: &str,
    cause: &str,
    head: Option<&[u8]>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    eprintln!(
        "stado resolver service={} consumer={} endpoint={} refused connection: {}",
        adapter.service, adapter.consumer, endpoint, cause
    );
    let mut sniff = [0_u8; 512];
    let head = match head {
        Some(head) => Some(head),
        None => match tokio::time::timeout(REFUSAL_SNIFF, reader.read(&mut sniff)).await {
            Ok(Ok(read)) if read > 0 => Some(&sniff[..read]),
            _ => None,
        },
    };
    if head.is_some_and(http_request_head) {
        let _ = writer.write_all(UPSTREAM_REFUSAL).await;
    }
    let _ = writer.shutdown().await;
}

async fn proxy_connection(
    client: TcpStream,
    adapter: &ResolverAdapter,
    state: &ResolverState,
) -> Result<(), String> {
    let resolved = state.resolve(&adapter.service, &adapter.consumer).await?;
    eprintln!(
        "stado resolver service={} consumer={} generation={} destination={}",
        adapter.service, adapter.consumer, resolved.generation, resolved.active_host
    );
    let endpoint = url::Url::parse(&resolved.endpoint.url)
        .map_err(|error| format!("invalid resolved endpoint: {error}"))?;
    let host = endpoint
        .host_str()
        .ok_or_else(|| "resolved endpoint has no host".to_string())?;
    let port = endpoint
        .port_or_known_default()
        .ok_or_else(|| "resolved endpoint has no port".to_string())?;
    let idle = Duration::from_secs(adapter.idle_seconds);
    // The directory is supposed to name where a service listens. When it names
    // this adapter's own bind instead, the proxy dials itself: the connection is
    // accepted, forwarded to the same socket, accepted again, and the reader
    // waits on a chain that never reaches a server. It looks exactly like a
    // healthy port with a hung backend, so say it instead of recursing.
    if format!("{host}:{port}") == adapter.bind {
        return Err(format!(
            "service {} resolves to {}, which is this adapter's own bind: the service \
             directory must carry the address the service listens on, not the stable \
             port published for its clients",
            adapter.service, adapter.bind
        ));
    }
    // Both halves of the fleet reach the upstream over a plain socket: the host
    // that holds the store dials it directly, every other host dials the
    // forward this resolver keeps to it. One code path, and neither branch
    // below starts a process.
    //
    // That last sentence was false for as long as it stood here, and it is
    // worth saying why rather than trusting it again. It read "no process per
    // request on either" while the `else` branch called
    // `select_resolver_ssh_path` right here, ahead of the pool: every host
    // that declares an `ssh_fallbacks` entry -- every host in this fleet --
    // paid one `ssh <destination> true`, bounded at twenty seconds, in front
    // of every accepted connection. The claim was about the forward and was
    // written as though it covered the whole function.
    //
    // So it is now checkable against what is visible below: this branch dials
    // and nothing else. Every process the transport needs -- the path probe
    // and the forward itself -- is started by
    // [`ResolverState::tunnel_connect`], under the pool lock, once per
    // forward. A request that finds a warm forward costs the two sockets it
    // would cost anyway.
    let (mut client_read, mut client_write) = client.into_split();
    let upstream = if resolved.active_host == state.local_target {
        TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("local upstream connect failed: {error}"))
    } else {
        let paths = resolved_ssh_paths(&resolved);
        state
            .tunnel_connect(&resolved.active_host, &paths, host, port)
            .await
            .map_err(|error| format!("active host {:?}: {error}", resolved.active_host))
    };
    // A refusal is written to the client before anything is read from it, so
    // the sentence names the reason instead of a socket that closed silently.
    let upstream = match upstream {
        Ok(upstream) => upstream,
        Err(cause) => {
            refuse_connection(
                &mut client_read,
                &mut client_write,
                adapter,
                &format!("{host}:{port}"),
                &cause,
                None,
            )
            .await;
            return Ok(());
        }
    };
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
    let activity = Activity::new();
    // What a client reads instead of a silent close: the service that did not
    // answer, and the window it was measured against. `Connection: close` and
    // an exact length, so a client library parses it as a complete message.
    let body = format!(
        "service {} did not answer within the {}s idle window this adapter declares\n",
        adapter.service,
        idle.as_secs()
    );
    let timeout_answer = format!(
        "HTTP/1.1 504 Gateway Timeout\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let upload = copy_until_idle(&mut client_read, &mut upstream_write, idle, &activity, None);
    let download = copy_until_idle(
        &mut upstream_read,
        &mut client_write,
        idle,
        &activity,
        Some(timeout_answer.as_bytes()),
    );
    let (sent, received) =
        tokio::try_join!(upload, download).map_err(|error| format!("proxy failed: {error}"))?;
    // A cut connection is the proxy's decision, and a client that received a
    // truncated answer has to be able to read whose decision it was and after
    // how long. Silence here is what made `connection closed before message
    // completed` unattributable for four release runs.
    if sent.cut || received.cut {
        eprintln!(
            "stado resolver service={} consumer={} endpoint={host}:{port} closed an idle \
             connection after {}s with nothing moving in either direction: {} byte(s) to the \
             service, {} byte(s) back",
            adapter.service,
            adapter.consumer,
            idle.as_secs(),
            sent.bytes,
            received.bytes,
        );
    }
    Ok(())
}

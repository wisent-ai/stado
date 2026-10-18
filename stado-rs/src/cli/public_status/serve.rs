//! `stado public-status serve --bind ADDRESS` — the status page itself.
//!
//! Every request reads the published document from the object store again,
//! so the page says what the last check found, and when the document cannot
//! be read the page answers `503 Service Unavailable` with the reason in the
//! store's own words instead of an old or empty page. It runs as its own unit,
//! on a host apart from the products it reports on, so it answers while they
//! do not. `GET /` is the page, `GET /status.json` the document as Stado
//! published it (or `{"error": …}` beside a 503); anything else is 404, and a
//! method other than GET or HEAD is 405.

use reqwest::{Method, StatusCode, Version};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use super::{document, page};
use crate::cli::CmdError;
use crate::queue::JobStorage;

/// The document's address beside the page's.
const DOCUMENT_PATH: &str = "/status.json";
const HTML: &str = "text/html; charset=utf-8";
const JSON: &str = "application/json";
const TEXT: &str = "text/plain; charset=utf-8";

/// One response: status, content type and body.
struct Answer {
    status: StatusCode,
    kind: &'static str,
    body: String,
}

/// What `GET target` answers.
async fn answer(store: &JobStorage, target: &str) -> Answer {
    let read = match document::read(store).await {
        Ok(Some(published)) => Ok(published),
        Ok(None) => Err(format!(
            "no status is published at {} in the object store: no `stado public-status \
             check` has run against it",
            document::BLOB
        )),
        Err(error) => Err(error.to_string()),
    };
    match (target, read) {
        ("/", Ok(published)) => Answer {
            status: StatusCode::OK,
            kind: HTML,
            body: page::published(&published),
        },
        ("/", Err(cause)) => Answer {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: HTML,
            body: page::unread(&cause),
        },
        (DOCUMENT_PATH, Ok(published)) => Answer {
            status: StatusCode::OK,
            kind: JSON,
            body: published.to_string(),
        },
        (DOCUMENT_PATH, Err(cause)) => Answer {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: JSON,
            body: json!({ "error": cause }).to_string(),
        },
        (other, _) => Answer {
            status: StatusCode::NOT_FOUND,
            kind: TEXT,
            body: format!("{other} is not a page of this status site; the page is /\n"),
        },
    }
}

/// Read the request through the API listener's own reader, with its declared
/// or measured limits, answer it, close. A client that closed before sending
/// a byte is answered with nothing.
async fn respond(
    store: JobStorage,
    mut stream: TcpStream,
    limits: crate::dashboard::RequestLimits,
) -> Result<(), String> {
    let mut carry = Vec::new();
    let read = crate::dashboard::read_request(&mut stream, &mut carry, limits).await;
    let (head_only, answer) = match read {
        Ok(None) => return Ok(()),
        Ok(Some(request)) => {
            let method = Method::from_bytes(request.method.as_bytes());
            let answer = match &method {
                Ok(method) if method == Method::GET || method == Method::HEAD => {
                    answer(&store, &request.path).await
                }
                Ok(_) => Answer {
                    status: StatusCode::METHOD_NOT_ALLOWED,
                    kind: TEXT,
                    body: "this status site answers GET and HEAD\n".to_owned(),
                },
                Err(error) => Answer {
                    status: StatusCode::BAD_REQUEST,
                    kind: TEXT,
                    body: format!("{:?} is no HTTP method: {error}\n", request.method),
                },
            };
            (
                matches!(method, Ok(method) if method == Method::HEAD),
                answer,
            )
        }
        Err(cause) => (
            false,
            Answer {
                status: StatusCode::BAD_REQUEST,
                kind: TEXT,
                body: format!("{cause}\n"),
            },
        ),
    };
    let reason = match answer.status.canonical_reason() {
        Some(reason) => reason,
        None => answer.status.as_str(),
    };
    let head = format!(
        "{:?} {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: \
         no-store\r\nConnection: close\r\n\r\n",
        Version::HTTP_11,
        answer.status.as_str(),
        answer.kind,
        answer.body.len()
    );
    let stream = &mut stream;
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| format!("the answer could not be written: {error}"))?;
    if !head_only {
        stream
            .write_all(answer.body.as_bytes())
            .await
            .map_err(|error| format!("the answer could not be written: {error}"))?;
    }
    stream
        .shutdown()
        .await
        .map_err(|error| format!("the connection could not be closed: {error}"))
}

/// Serve the page on `bind` until the process is ended. A connection the
/// client gave up before it was accepted is said and the next is taken; any
/// other accept failure (descriptors or memory exhausted) ends the serve with
/// its cause, as accepting again would fail the same way at once.
pub(super) async fn serve(bind: &str) -> Result<(), CmdError> {
    let limits = crate::dashboard::RequestLimits::read()
        .map_err(|error| CmdError::refused(format!("public-status request limits: {error}")))?;
    let store = JobStorage::with_bucket(crate::config::bucket()).await?;
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| CmdError::click(format!("cannot listen on {bind}: {error}")))?;
    let address = listener.local_addr().map_err(|error| {
        CmdError::click(format!(
            "the address bound for {bind} cannot be read: {error}"
        ))
    })?;
    println!("stado public-status: the status page listens on {address}");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                eprintln!("stado public-status: a client gave up before it was accepted: {error}");
                continue;
            }
            Err(error) => {
                return Err(CmdError::click(format!(
                    "stado public-status on {address} cannot accept connections: {error}"
                )))
            }
        };
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(error) = respond(store, stream, limits).await {
                eprintln!("stado public-status: {peer}: {error}");
            }
        });
    }
}

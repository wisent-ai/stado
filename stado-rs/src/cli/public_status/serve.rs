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

use reqwest::{StatusCode, Version};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// Read the request head, answer it, close.
async fn respond(store: JobStorage, stream: TcpStream) -> Result<(), String> {
    let mut stream = BufReader::new(stream);
    let mut request_line = String::new();
    stream
        .read_line(&mut request_line)
        .await
        .map_err(|error| format!("the request could not be read: {error}"))?;
    // The head ends at its first empty line, or where the client stopped.
    loop {
        let mut header = String::new();
        stream
            .read_line(&mut header)
            .await
            .map_err(|error| format!("a request header could not be read: {error}"))?;
        if header.trim().is_empty() {
            break;
        }
    }
    let mut words = request_line.split_whitespace();
    let (method, target) = (words.next(), words.next());
    let head_only = method == Some("HEAD");
    let answer = match (method, target) {
        (Some("GET" | "HEAD"), Some(target)) => answer(&store, target).await,
        (Some(_), Some(_)) => Answer {
            status: StatusCode::METHOD_NOT_ALLOWED,
            kind: TEXT,
            body: "this status site answers GET and HEAD\n".to_owned(),
        },
        _ => Answer {
            status: StatusCode::BAD_REQUEST,
            kind: TEXT,
            body: format!("{:?} is not an HTTP request line\n", request_line.trim()),
        },
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
    let stream = stream.get_mut();
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

/// Serve the page on `bind` until the process is ended.
pub(super) async fn serve(bind: &str) -> Result<(), CmdError> {
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
            Err(error) => {
                eprintln!("stado public-status: a connection could not be accepted: {error}");
                continue;
            }
        };
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(error) = respond(store, stream).await {
                eprintln!("stado public-status: {peer}: {error}");
            }
        });
    }
}

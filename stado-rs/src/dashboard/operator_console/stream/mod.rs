//! One authenticated WebSocket owns one interactive workload attachment.
//! Text input is UTF-8 stdin; binary input is byte-exact stdin. Server binary
//! frames prefix stdout with 1 and stderr with 2; text frames carry exit/errors.

mod bridge;

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{
        handshake::server,
        http,
        protocol::{Role, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

use super::{
    default_timeout, operator_auth, send_json, validate, Request, Response, RunRequest,
    MAX_REQUEST_BYTES, MUTATION_CONFIRMATION, STATUS_BAD_REQUEST, STATUS_FORBIDDEN,
    STATUS_UNAUTHORIZED, STATUS_UNAVAILABLE,
};

pub(crate) const PATH: &str = "/api/operator/workload/attach";
pub(super) type Socket = WebSocketStream<TcpStream>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentRequest {
    kind: String,
    target: Option<String>,
    workspace: Option<String>,
    resume: Option<String>,
    #[serde(default)]
    confirmation: String,
}

impl AttachmentRequest {
    fn command(self) -> Result<RunRequest, String> {
        if self.confirmation != MUTATION_CONFIRMATION {
            return Err("workload attachment requires explicit RUN_MUTATION confirmation".into());
        }
        let mut args = vec!["workload".into(), "attach".into(), self.kind];
        for (flag, value) in [
            ("--target", self.target),
            ("--workspace", self.workspace),
            ("--resume", self.resume),
        ] {
            if let Some(value) = value {
                args.extend([flag.into(), value]);
            }
        }
        let request = RunRequest {
            args,
            input: None,
            stdin: None,
            confirmation: self.confirmation,
            timeout_seconds: default_timeout(),
        };
        validate(&request).map_err(|error| error.message)?;
        Ok(request)
    }
}

pub(crate) async fn upgrade(request: &Request) -> Response {
    if request.path != PATH || request.header("x-stado-action") != Some("workload-attach") {
        return send_json(
            STATUS_FORBIDDEN,
            &json!({"ok": false, "error": "forbidden"}),
        );
    }
    match operator_auth::authorized(request).await {
        Ok(true) => {}
        Ok(false) => {
            return send_json(
                STATUS_UNAUTHORIZED,
                &json!({"ok": false, "error": "unauthorized"}),
            )
        }
        Err(error) => {
            return send_json(
                STATUS_UNAVAILABLE,
                &json!({"ok": false, "error": error.to_string()}),
            )
        }
    }
    if request.version != "HTTP/1.1" || !request.body.is_empty() {
        return send_json(
            STATUS_BAD_REQUEST,
            &json!({"ok": false, "error": "invalid WebSocket request framing"}),
        );
    }
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(&request.path);
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let response = builder
        .body(())
        .map_err(|error| error.to_string())
        .and_then(|request| server::create_response(&request).map_err(|error| error.to_string()));
    let response = match response {
        Ok(response) => response,
        Err(error) => return send_json(STATUS_BAD_REQUEST, &json!({"ok": false, "error": error})),
    };
    let mut bytes = b"HTTP/1.1 101 Switching Protocols\r\n".to_vec();
    for (name, value) in response.headers() {
        bytes.extend_from_slice(name.as_str().as_bytes());
        bytes.extend_from_slice(b": ");
        bytes.extend_from_slice(value.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"\r\n");
    Response {
        status: response.status().as_u16(),
        bytes,
    }
}

async fn setup(socket: &mut Socket) -> Result<Option<RunRequest>, String> {
    while let Some(message) = socket.next().await {
        match message.map_err(|error| error.to_string())? {
            Message::Text(text) => {
                let request: AttachmentRequest = serde_json::from_str(&text)
                    .map_err(|error| format!("invalid attachment request: {error}"))?;
                return request.command().map(Some);
            }
            Message::Close(_) => return Ok(None),
            Message::Ping(_) | Message::Pong(_) => {
                socket.flush().await.map_err(|error| error.to_string())?
            }
            _ => return Err("the first workload frame must be a JSON attachment request".into()),
        }
    }
    Ok(None)
}

pub(super) async fn error(socket: &mut Socket, detail: &str) {
    let _ = socket
        .send(Message::text(
            json!({"type": "error", "message": detail}).to_string(),
        ))
        .await;
}

pub(crate) async fn serve(stream: TcpStream, carry: Vec<u8>) -> std::io::Result<()> {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_REQUEST_BYTES);
    config.max_frame_size = Some(MAX_REQUEST_BYTES);
    let mut socket =
        WebSocketStream::from_partially_read(stream, carry, Role::Server, Some(config)).await;
    let request = tokio::time::timeout(
        std::time::Duration::from_secs(default_timeout()),
        setup(&mut socket),
    )
    .await;
    match request {
        Ok(Ok(Some(request))) => {
            if let Err(detail) = bridge::run(&mut socket, &request.args).await {
                error(&mut socket, &detail).await;
            }
        }
        Ok(Ok(None)) => return Ok(()),
        Ok(Err(detail)) => error(&mut socket, &detail).await,
        Err(_) => {
            error(
                &mut socket,
                "no attachment request arrived before the connection deadline",
            )
            .await
        }
    }
    let _ = socket.close(None).await;
    Ok(())
}

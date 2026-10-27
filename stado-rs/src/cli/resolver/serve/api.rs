use std::collections::BTreeMap;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::cli::resolver::serve::state::ResolverState;

/// Where the resolver API answers the registry generation it holds.
pub(in crate::cli::resolver) const SNAPSHOT_PATH: &str = "/v1/registry/snapshot";

pub(super) async fn serve_api(
    listener: TcpListener,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("resolution API accept failed: {error}"))?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let response = match read_request(&mut stream).await {
                Ok(request) => handle_api_request(request, &state).await,
                Err(error) => api_response(StatusCode::BAD_REQUEST, json!({"error": error})),
            };
            if let Err(error) = stream.write_all(&response).await {
                eprintln!("stado resolver API write failed: {error}");
            }
        });
    }
}

struct ApiRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
}

/// The request line and headers, read line by line up to the blank line that
/// ends an HTTP head; the API takes no body.
async fn read_request(stream: &mut TcpStream) -> Result<ApiRequest, String> {
    let mut reader = BufReader::new(stream);
    let mut head: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("request read failed: {error}"))?;
        if line.is_empty() {
            return Err("request ended before HTTP head".to_string());
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        head.push(line);
    }
    let mut lines = head.into_iter();
    let request_line = lines
        .next()
        .ok_or_else(|| "request line is missing".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| "request method is missing".to_string())?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| "request path is missing".to_string())?
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("request header {line:?} has no colon"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(ApiRequest {
        method,
        path,
        headers,
    })
}

async fn handle_api_request(request: ApiRequest, state: &ResolverState) -> Vec<u8> {
    if request.method != "GET" {
        return api_response(StatusCode::METHOD_NOT_ALLOWED, json!({"error": "method_not_allowed"}));
    }
    if request.path == "/health" {
        let current = state.snapshot.read().await;
        if current.loaded_at.elapsed() > state.max_stale {
            return api_response(StatusCode::SERVICE_UNAVAILABLE, json!({"status": "stale"}));
        }
        return api_response(
            StatusCode::OK,
            json!({
                "status": "ok",
                "service": "stado-resolver",
                "generation": current.directory_generation,
            }),
        );
    }
    // The generation this process holds, for `stado resolver snapshot`.
    if request.path == SNAPSHOT_PATH {
        return match state.held_snapshot().await {
            Ok(payload) => api_response(StatusCode::OK, payload),
            Err(error) => api_response(StatusCode::SERVICE_UNAVAILABLE, json!({"error": error})),
        };
    }
    let Some(service) = request.path.strip_prefix("/v1/resolve/service/") else {
        return api_response(StatusCode::NOT_FOUND, json!({"error": "not_found"}));
    };
    if service.is_empty() || service.contains('/') || service.contains('?') {
        return api_response(StatusCode::BAD_REQUEST, json!({"error": "invalid_service"}));
    }
    let Some(consumer) = request.headers.get("x-stado-consumer") else {
        return api_response(StatusCode::UNAUTHORIZED, json!({"error": "consumer_required"}));
    };
    match state.resolve(service, consumer).await {
        Ok(resolved) => api_response(
            StatusCode::OK,
            json!({
                "service": format!("stado://service/{}", resolved.name),
                "generation": resolved.generation,
                "gateway_url": state.gateway_url(service, consumer),
                "capabilities": resolved.capabilities,
            }),
        ),
        Err(error) => api_response(StatusCode::SERVICE_UNAVAILABLE, json!({"error": error})),
    }
}

/// One whole HTTP/1.1 answer: the status with its own reason phrase, and the
/// JSON body.
fn api_response(status: StatusCode, body: Value) -> Vec<u8> {
    let body = body.to_string().into_bytes();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(&body);
    response
}

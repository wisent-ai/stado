use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::cli::resolver::serve::proxy::accept_backoff;
use crate::cli::resolver::serve::proxy::ACCEPT_FAILURE_LIMIT;
use crate::cli::resolver::serve::state::ResolverState;

const REQUEST_HEAD_LIMIT: usize = 16 * 1024;

pub(super) async fn serve_api(
    listener: TcpListener,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    let mut failures = 0_u32;
    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(accepted) => {
                failures = 0;
                accepted
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures >= ACCEPT_FAILURE_LIMIT {
                    return Err(format!(
                        "resolution API accept failed {failures} times in a row: {error}"
                    ));
                }
                accept_backoff("resolution API", &error, failures).await;
                continue;
            }
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let response = match read_request(&mut stream).await {
                Ok(request) => handle_api_request(request, &state).await,
                Err(error) => api_response(400, json!({"error": error})),
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

async fn read_request(stream: &mut TcpStream) -> Result<ApiRequest, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("request read failed: {error}"))?;
        if read == 0 {
            return Err("request ended before HTTP head".to_string());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > REQUEST_HEAD_LIMIT {
            return Err("request head is too large".to_string());
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8(bytes).map_err(|_| "request head is not UTF-8".to_string())?;
    let mut lines = head.split("\r\n");
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
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "invalid request header".to_string())?;
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
        return api_response(405, json!({"error": "method_not_allowed"}));
    }
    if request.path == "/health" {
        let current = state.snapshot.read().await;
        if current.loaded_at.elapsed() > state.max_stale {
            return api_response(503, json!({"status": "stale"}));
        }
        return api_response(
            200,
            json!({
                "status": "ok",
                "service": "stado-resolver",
                "generation": current.directory_generation,
            }),
        );
    }
    let Some(service) = request.path.strip_prefix("/v1/resolve/service/") else {
        return api_response(404, json!({"error": "not_found"}));
    };
    if service.is_empty() || service.contains('/') || service.contains('?') {
        return api_response(400, json!({"error": "invalid_service"}));
    }
    let Some(consumer) = request.headers.get("x-stado-consumer") else {
        return api_response(401, json!({"error": "consumer_required"}));
    };
    match state.resolve(service, consumer).await {
        Ok(resolved) => api_response(
            200,
            json!({
                "service": format!("stado://service/{}", resolved.name),
                "generation": resolved.generation,
                "gateway_url": state.gateway_url(service, consumer),
                "capabilities": resolved.capabilities,
            }),
        ),
        Err(error) => api_response(503, json!({"error": error})),
    }
}

fn api_response(status: u16, body: Value) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    };
    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(&body);
    response
}

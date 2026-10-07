//! Real WebSocket attachments against the isolated product process. The help
//! invocation exercises accepted setup without starting a fleet workload.
use super::fixture::Service;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    client_async,
    tungstenite::{client::IntoClientRequest, http::StatusCode, Message},
};

fn attachment(kind: &str) -> Value {
    json!({"kind": kind, "confirmation": "RUN_MUTATION"})
}

fn limits(request_bytes: usize) -> Value {
    let declaration = std::env::var("STADO_TEST_REQUEST_LIMITS")
        .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API bounds");
    let mut limits: Value = serde_json::from_str(&declaration).unwrap();
    let arguments = ["workload", "attach", "--help"];
    let console = limits["operator_console"]
        .as_object_mut()
        .expect("STADO_TEST_REQUEST_LIMITS must declare operator_console bounds");
    console.insert("argument_count".into(), json!(arguments.len()));
    console.insert(
        "argument_bytes".into(),
        json!(arguments
            .iter()
            .map(|argument| argument.len())
            .max()
            .unwrap()),
    );
    console.insert("request_bytes".into(), json!(request_bytes));
    limits
}

async fn exchange(service: &mut Service, body: &str) -> Vec<Value> {
    let address = service.origin.strip_prefix("http://").unwrap();
    let endpoint = format!("ws://{address}/api/operator/workload/attach");
    let mut request = endpoint.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("x-stado-action", "workload-attach".parse().unwrap());
    service.observe(
        "attachment_request",
        json!({"endpoint": endpoint, "body": body}),
    );
    let stream = TcpStream::connect(request.uri().authority().unwrap().as_str())
        .await
        .unwrap();
    let handshake = client_async(request, stream).await;
    if let Err(error) = &handshake {
        service.observe("handshake_error", json!(error.to_string()));
    }
    let (mut socket, response) = handshake.unwrap();
    service.observe("handshake_status", json!(response.status().as_u16()));
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket.send(Message::text(body)).await.unwrap();
    let mut messages = Vec::new();
    while let Some(result) = socket.next().await {
        if let Err(error) = &result {
            service.observe("transport_error", json!(error.to_string()));
        }
        match result.unwrap() {
            Message::Text(text) => {
                service.observe(
                    &format!("text_frame_{}", messages.len()),
                    json!(text.as_str()),
                );
                messages.push(serde_json::from_str(&text).unwrap());
            }
            Message::Binary(bytes) => {
                service.observe("command_output", json!(bytes.as_ref()));
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => socket.flush().await.unwrap(),
            other => panic!("unexpected server frame: {other:?}"),
        }
    }
    messages
}

#[tokio::test]
async fn attachment_at_declared_bounds_executes_the_command() {
    let body = attachment("--help").to_string();
    let mut service = Service::start_with_request_limits(&limits(body.len()));
    let before = service.persisted();
    let messages = exchange(&mut service, &body).await;
    assert_eq!(
        messages.first().unwrap()["type"],
        "attached",
        "{messages:?}"
    );
    let exit = messages.last().unwrap();
    assert_eq!(exit["type"], "exit", "{messages:?}");
    assert_eq!(exit["ok"], true, "{messages:?}");
    let after = service.persisted();
    service.observe("configuration_after", after.clone());
    assert_eq!(after, before, "help changed persisted configuration");
    service.pass();
}

#[tokio::test]
async fn attachment_refuses_expanded_arguments_and_oversized_frames() {
    let ordinary = attachment("--help");
    let mut too_many = ordinary.clone();
    too_many["target"] = json!("target");
    let oversized = format!("{ordinary} ");
    let cases = [
        (too_many.to_string(), "argument_count", None),
        (
            attachment("--help-extra").to_string(),
            "argument_bytes",
            None,
        ),
        (attachment("ééééé").to_string(), "argument_bytes", None),
        (oversized, "", Some(ordinary.to_string().len())),
    ];
    for (body, field, message_bound) in cases {
        let bound = match message_bound {
            Some(bound) => bound,
            None => body.len(),
        };
        let mut service = Service::start_with_request_limits(&limits(bound));
        let before = service.persisted();
        let messages = exchange(&mut service, &body).await;
        assert!(
            !messages.iter().any(|message| message["type"] == "attached"),
            "{messages:?}"
        );
        let refusal = messages.first().unwrap();
        assert_eq!(refusal["type"], "error", "{messages:?}");
        let reason = refusal["message"].as_str().unwrap();
        if message_bound.is_some() {
            assert!(reason.contains(&body.len().to_string()), "{reason}");
            assert!(reason.contains(&bound.to_string()), "{reason}");
        } else {
            assert!(
                reason.contains(&format!(
                    "dashboard.request_limits.operator_console.{field}"
                )),
                "{reason}"
            );
        }
        let after = service.persisted();
        service.observe("configuration_after", after.clone());
        assert_eq!(
            after, before,
            "refused attachment changed persisted configuration"
        );
        service.pass();
    }
}

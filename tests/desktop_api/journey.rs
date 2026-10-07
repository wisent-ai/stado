//! Real API and CLI reads of one isolated declaration, with persisted state
//! checks. This does not qualify database provisioning or graphical surfaces.
mod configuration;
mod console_limits;
mod fixture;
#[cfg(unix)]
mod registry_inputs;

use fixture::Service;
use serde_json::{json, Value};

fn payload(receipt: Value) -> Value {
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert_eq!(receipt["exit_code"], 0, "{receipt}");
    assert_eq!(receipt["stdout_truncated"], false, "{receipt}");
    assert_eq!(receipt["stderr_truncated"], false, "{receipt}");
    serde_json::from_str(receipt["stdout"].as_str().unwrap()).unwrap()
}

fn declaration(scopes: &str) -> Value {
    json!({"args": ["database", "declare", "native-api-journey", "--engine", "postgres", "--scope", scopes, "--consumer", "native-api-journey", "--json"]})
}

fn listing() -> Value {
    json!({"args": ["database", "list", "--json"]})
}
fn removal() -> Value {
    json!({"args": ["database", "remove", "native-api-journey", "--json"]})
}
fn confirmed(mut request: Value) -> Value {
    request["confirmation"] = json!("RUN_MUTATION");
    request
}
fn row(list: &Value) -> Option<&Value> {
    list.as_array()
        .unwrap()
        .iter()
        .find(|row| row["database"] == "native-api-journey")
}

#[tokio::test]
async fn native_api_preserves_confirmation_refusals_and_durable_declaration_changes() {
    let mut service = Service::start();
    let name = "native-api-journey";
    let before = service.persisted();
    service.call(declaration("read"), 403).await;
    assert_eq!(
        service.persisted(),
        before,
        "an unconfirmed mutation wrote the declaration"
    );
    assert!(row(&payload(service.call(listing(), 200).await)).is_none());

    payload(service.call(confirmed(declaration("read")), 200).await);
    let saved = service.persisted();
    assert_eq!(
        saved["database_api"]["databases"][name]["engine"],
        "postgres"
    );
    assert_eq!(
        saved["database_api"]["databases"][name]["scopes"],
        json!(["read"])
    );
    let listed = payload(service.call(listing(), 200).await);
    assert_eq!(row(&listed).unwrap()["engine"], "postgres");

    payload(
        service
            .call(confirmed(declaration("read,write")), 200)
            .await,
    );
    assert_eq!(
        service.persisted()["database_api"]["databases"][name]["scopes"],
        json!(["read", "write"])
    );
    let independent: Value =
        serde_json::from_str(&service.cli(&["database", "list", "--json"])).unwrap();
    assert_eq!(
        row(&independent).unwrap()["scopes"],
        json!(["read", "write"])
    );

    let before = service.persisted();
    let refusal = service
        .call(confirmed(declaration("invalid-scope")), 200)
        .await;
    assert_eq!(refusal["ok"], false, "{refusal}");
    assert_ne!(
        refusal["exit_code"]
            .as_i64()
            .expect("the real refusal exit code"),
        0
    );
    assert_eq!(
        service.persisted(),
        before,
        "invalid input changed the prior declaration"
    );

    service.call(removal(), 403).await;
    assert_eq!(
        service.persisted(),
        before,
        "unconfirmed removal changed the declaration"
    );
    payload(service.call(confirmed(removal()), 200).await);
    assert!(service.persisted()["database_api"]["databases"]
        .get(name)
        .is_none());
    assert!(row(&payload(service.call(listing(), 200).await)).is_none());
    let independent: Value =
        serde_json::from_str(&service.cli(&["database", "list", "--json"])).unwrap();
    assert!(row(&independent).is_none());
    service.pass();
}

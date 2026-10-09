//! An object request that would stand behind the vault is refused with the
//! server's own wait line, never held.
//!
//! Two journeys on the deployment in [`fixture`]:
//!
//! 1. The keyring's lock is really held — the keyboxd database's lock file
//!    carries the pid of a live process of this test, so gpg waits on it —
//!    and the item is rotated, so the next request must read the bearer. That
//!    request stands on the vault (its own `czekam` line in the listener's
//!    log); a second request, sent while it stands, is answered 503 at once
//!    with that `czekam` line as `cause`, and `/api/state.json` carries the
//!    same line as the namespace's open consultation. The holder is ended,
//!    gpg takes the lock from the dead pid, the first request is answered
//!    Not Found (an absent object, which only an authorized request reaches)
//!    and the consultation is closed again.
//! 2. The grant is revoked, so the vault refuses the version read: the
//!    request is answered 503 with the `blad czekania` line naming the 403;
//!    the namespace's `failed` line in `/api/state.json` is that line; the
//!    grant is issued again and the next request is authorized, which clears
//!    the failure.
mod fixture;

use serde_json::{json, Value};
use std::process::{Command, Stdio};

use fixture::{
    get_object, hold_keyring_lock, keyboxd_lock, Deployment, CONSUMER, ITEM, NAMESPACE, URI,
};

const UNAVAILABLE: &str = "object authorization unavailable";

/// The cause a 503 carries: the server's own wait line.
fn cause_of(body: &Value) -> &str {
    assert_eq!(
        body["error"].as_str(),
        Some(UNAVAILABLE),
        "a request the authority could not answer for is refused as unavailable: {body}"
    );
    match body["cause"].as_str() {
        Some(cause) => cause,
        None => panic!("a 503 names its cause, the server's own wait line: {body}"),
    }
}

#[tokio::test]
async fn a_request_behind_a_held_vault_is_refused_with_the_servers_wait_line() {
    let mut deployment = Deployment::start();
    let first = format!("first-{}", uuid::Uuid::new_v4().simple());
    let second = format!("second-{}", uuid::Uuid::new_v4().simple());

    let mut init = deployment.skarbiec();
    init.args(["init", "consultation-probe-owner"]);
    deployment.must("skarbiec init", init);
    deployment.set_token(&first);
    let token_file = deployment.root.join("verifier.token");
    deployment.issue_grant(&token_file);

    let mut vault = deployment.skarbiec();
    vault.args(["serve", "--port", "0"]);
    let vault_address = deployment.spawn("skarbiec", vault, "listening on http://");

    let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
        .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
    let mut init = deployment.stado();
    init.args(["config", "init"]);
    deployment.must("stado config init", init);
    let mut set = deployment.stado();
    set.args(["config", "set", "dashboard.request_limits", &limits]);
    deployment.must("stado config set dashboard.request_limits", set);

    let namespaces = json!({
        NAMESPACE: {"item": ITEM, "prefix_policies": [{"prefix": "probe/", "actions": ["get"]}]}
    });
    let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
    let mut api = deployment.stado();
    api.env("WC_SKARBIEC_URL", format!("http://{vault_address}"))
        .env("WC_SKARBIEC_CONSUMER", CONSUMER)
        .env("WC_SKARBIEC_TOKEN_FILE", &token_file)
        .env("WC_OBJECT_API_NAMESPACES", namespaces.to_string())
        .args([
            "serve",
            "--api",
            "--bind",
            &loopback,
            "--port",
            "0",
            "--api-local-store",
        ])
        .arg(deployment.root.join("store"));
    let api_address = deployment.spawn("stado", api, "[dashboard] listening on http://");
    let origin = format!("http://{api_address}");

    // The bearer is read once and held with its version.
    let (status, _) = deployment.get(&origin, &first).await;
    assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "the first bearer must be accepted (an absent object answers Not Found only to an authorized request)"
    );
    let state = deployment.state(&origin).await;
    assert_eq!(
        state["vault"]["consultations"][NAMESPACE],
        json!({"open": null, "failed": null}),
        "a namespace whose consultations succeeded stands on nothing: {state}"
    );

    // Journey 1: the keyring's lock is held by a live process and the item
    // moves, so the next request must read the bearer through gpg.
    deployment.set_token(&second);
    let mut holder = Command::new("sleep")
        .arg("3600")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    hold_keyring_lock(&deployment.keyring, &holder);
    let standing = tokio::spawn(get_object(origin.clone(), second.clone()));
    // The standing request has claimed the namespace once its czekam line
    // for the bearer read is the namespace's open consultation.
    let open = loop {
        let state = deployment.state(&origin).await;
        if let Some(open) = state["vault"]["consultations"][NAMESPACE]["open"].as_str() {
            break open.to_string();
        }
        assert!(
            !standing.is_finished(),
            "the request on the held keyring ended before it claimed the namespace: {state}"
        );
        tokio::task::yield_now().await;
    };
    assert!(
        open.starts_with(&format!(
            "czekam: the bearer of namespace {NAMESPACE} (item {ITEM}); gdzie: Skarbiec; rodzaj: siec; od: "
        )),
        "the open consultation is the bearer read's own czekam line: {open}"
    );
    let (status, body) = deployment.get(&origin, &second).await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "a request that would stand behind the bearer read is refused at once: {body}"
    );
    assert_eq!(
        cause_of(&body),
        open,
        "the refusal carries the czekam line of the consultation it would have stood behind"
    );
    assert!(
        !standing.is_finished(),
        "the standing request still waits on the held keyring while its peer was refused"
    );
    // The holder ends; the next gpg finds a dead pid in the lock and takes it.
    holder.kill().unwrap();
    holder.wait().unwrap();
    let (status, body) = standing.await.unwrap();
    deployment.record(
        &format!("GET /api/object?uri={URI} (stood on the held keyring)"),
        status,
        &body,
    );
    assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "the standing request is answered once the vault answers: {body}"
    );
    let state = deployment.state(&origin).await;
    assert_eq!(
        state["vault"]["consultations"][NAMESPACE],
        json!({"open": null, "failed": null}),
        "a consultation that ended well leaves the namespace standing on nothing: {state}"
    );
    assert!(
        !keyboxd_lock(&deployment.keyring).exists(),
        "gpg removed the dead holder's lock on its way through"
    );

    // Journey 2: the grant is revoked, so the vault refuses the version read.
    let mut revoke = deployment.skarbiec();
    revoke.args(["grant", "revoke", CONSUMER]);
    deployment.must("skarbiec grant revoke", revoke);
    let (status, body) = deployment.get(&origin, &second).await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "a request whose consultation the vault refused is answered unavailable: {body}"
    );
    let failed = cause_of(&body).to_string();
    assert!(
        failed.starts_with(&format!(
            "blad czekania: the version of the bearer of namespace {NAMESPACE} (item {ITEM}); gdzie: Skarbiec; trwalo: "
        )) && failed.contains("przyczyna: ")
            && failed.contains("HTTP 403"),
        "the refusal carries the blad czekania line of the failed consultation, with the vault's 403: {failed}"
    );
    let state = deployment.state(&origin).await;
    assert_eq!(
        state["vault"]["consultations"][NAMESPACE]["failed"].as_str(),
        Some(failed.as_str()),
        "the namespace's failed consultation is that line: {state}"
    );
    let (status, body) = deployment.get(&origin, &second).await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "the next request consults again and is refused again while the grant is gone: {body}"
    );
    assert!(
        cause_of(&body).starts_with("blad czekania: the version of the bearer of namespace "),
        "its cause is its own consultation's failure line: {body}"
    );

    deployment.issue_grant(&token_file);
    let (status, body) = deployment.get(&origin, &second).await;
    assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "once the grant is back the next request consults, succeeds and is authorized: {body}"
    );
    let state = deployment.state(&origin).await;
    assert_eq!(
        state["vault"]["consultations"][NAMESPACE],
        json!({"open": null, "failed": null}),
        "a consultation that succeeded clears the namespace's failure: {state}"
    );
    assert!(
        state["vault"]["keyring_lock_repair"].is_null(),
        "this host declares no vault, so the listener ran no keyring lock repair: {state}"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}

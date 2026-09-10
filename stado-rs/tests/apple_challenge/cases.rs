//! The Apple challenge journeys against the registered host.
use super::*;

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_readiness_observes_the_registered_host_without_preparing_it() {
    let target = registered_host();
    let observed = status(&target).await;
    observed.assert_ready(&target);
    verify_native_api(&target, &observed, false).await;
}

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_only_preparation_preserves_other_gui_state_and_works_through_the_desktop_api() {
    let target = registered_host();
    let file = plan_file();
    let plan = file.to_str().expect("the plan path is UTF-8");
    let unknown = format!("probierz-apple-unknown-{}", uuid::Uuid::new_v4());
    let refusal = prepare(&unknown, plan).await;
    assert_eq!(refusal.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refusal.stderr).to_string();
    let sentence = format!("target '{unknown}' is not declared; add it to the canonical registry");
    assert!(said.contains(&sentence), "{said}");
    let before = status(&target).await;
    let prepared = report(&prepare(&target, plan).await);
    assert_eq!(prepared.error, None, "{prepared:#?}");
    let after_cli = status(&target).await;
    after_cli.assert_ready(&target);
    assert_eq!(after_cli.ssh_target, before.ssh_target);
    assert_eq!(
        after_cli.unrelated_state(),
        before.unrelated_state(),
        "Apple-only preparation changed unrelated GUI state"
    );
    verify_native_api(&target, &after_cli, true).await;
}

async fn verify_native_api(target: &str, after_cli: &Report, prepare: bool) {
    let work =
        PathBuf::from(std::env::var_os("HOME").expect("HOME is required")).join(".stado/work");
    std::fs::create_dir_all(&work).expect("create test work root");
    let isolated = tempfile::Builder::new()
        .prefix("apple-preparation-api-")
        .tempdir_in(work)
        .expect("create isolated API store");
    // A real second instance of this exact product binary, isolated store.
    let mut server = Command::new(stado_binary())
        .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", isolated.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start the exact Stado API binary");
    let mut lines = BufReader::new(server.stderr.take().expect("capture API stderr")).lines();
    let endpoint = timeout(Duration::from_secs(60), async {
        while let Some(line) = lines.next_line().await.expect("read API startup") {
            eprintln!("API {line}");
            if let Some(endpoint) = line.strip_prefix("[dashboard] listening on ") {
                return endpoint.to_string();
            }
        }
        panic!("Stado API exited before listening");
    })
    .await
    .expect("the Stado API must bind within its startup bound");
    let logs = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("API {line}");
        }
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(330))
        .build()
        .expect("build the API client");
    let health = http.get(format!("{endpoint}/healthz")).send().await;
    assert_eq!(health.expect("read API health").status(), StatusCode::OK);
    let read = json!({
        "args": ["workload", "status", "gui-automation", "--target", target, "--json"],
        "timeout_seconds": API_COMMAND_SECONDS
    });
    let (code, body) = call(&http, &endpoint, &read, "apple-api-readiness.json").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let result: Value = serde_json::from_str(&body).expect("decode API readiness result");
    assert_eq!(result["exit_code"], 0, "{result}");
    let observed: Report =
        serde_json::from_str(result["stdout"].as_str().expect("capture readiness stdout"))
            .expect("decode the actual host readiness receipt");
    observed.assert_ready(target);
    assert_eq!(
        observed.state(),
        after_cli.state(),
        "the read-only Desktop API changed observed host state"
    );
    // The plan travels as staged input, the way the native client sends it.
    let unknown = format!("probierz-apple-unconfirmed-{}", uuid::Uuid::new_v4());
    let arguments = |host: &str| {
        json!([
            "workload",
            "run",
            "gui-automation",
            "--target",
            host,
            "--plan",
            "$INPUT",
            "--json"
        ])
    };
    let unconfirmed = json!({"args": arguments(&unknown), "input": APPLE_ONLY_PLAN});
    let (code, body) = call(&http, &endpoint, &unconfirmed, "apple-api-refusal.json").await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{body}");
    let refused: Value = serde_json::from_str(&body).expect("read the actual API refusal");
    let sentence = "mutating commands require explicit RUN_MUTATION confirmation";
    assert_eq!(refused["error"], sentence);
    if prepare {
        let request = json!({
            "args": arguments(target), "input": APPLE_ONLY_PLAN,
            "confirmation": "RUN_MUTATION", "timeout_seconds": API_COMMAND_SECONDS
        });
        let (code, body) = call(&http, &endpoint, &request, "apple-api-preparation.json").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        let result: Value = serde_json::from_str(&body).expect("decode API result");
        assert_eq!(result["exit_code"], 0, "{result}");
        assert_eq!(result["ok"], true, "{result}");
        let reused: Report =
            serde_json::from_str(result["stdout"].as_str().expect("capture command stdout"))
                .expect("decode the actual preparation receipt");
        assert_eq!(
            reused.state().get("apple-challenge-helper"),
            Some(&"reused")
        );
        let after_api = status(target).await;
        after_api.assert_ready(target);
        assert_eq!(
            after_api.state(),
            after_cli.state(),
            "repeating preparation through the API changed observed host state"
        );
        assert_eq!(after_api.ssh_target, after_cli.ssh_target);
    }
    server
        .kill()
        .await
        .expect("stop only the isolated test API");
    server.wait().await.expect("reap the isolated API process");
    logs.await.expect("retain the remaining API log");
}
/// One operator-API request, with its status and complete body retained.
async fn call(
    http: &reqwest::Client,
    endpoint: &str,
    request: &Value,
    name: &str,
) -> (StatusCode, String) {
    let response = http
        .post(format!("{endpoint}/api/operator/run"))
        .header("X-Stado-Action", "operator-command")
        .json(request)
        .send()
        .await
        .expect("reach the native operator API");
    let status = response.status();
    let body = response.text().await.expect("retain the API response");
    retain(
        name,
        &json!({"request": request, "http_status": status.as_u16(), "body": body}),
    );
    (status, body)
}

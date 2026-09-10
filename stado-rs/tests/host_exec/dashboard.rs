//! The same fixed read, sent through a real Stado dashboard: it answers
//! without a mutation confirmation, and an exact provider sign-in still does
//! not, so a confirmation-classification regression stops before any
//! provider is contacted.

use std::process::Stdio;

use crate::story::SYSTEM_PATH;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::timeout;

use crate::journey::{write_private, Journey};
use crate::story::{PROVIDER_SIGN_IN, TARGET};

impl Journey {
    pub(crate) async fn verify_dashboard_boundary(&self) {
        let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_stado"));
        server
            .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            // The server consumes this isolated override itself. Its operator
            // child deliberately removes the override and reads the same path
            // from the isolated config document instead.
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "host-exec-retained-logs")
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut server = server
            .spawn()
            .expect("the built Stado dashboard API starts");
        let mut dashboard_stdout = server
            .stdout
            .take()
            .expect("capture the real dashboard stdout");
        let stdout_logs = tokio::spawn(async move {
            let mut retained = Vec::new();
            dashboard_stdout
                .read_to_end(&mut retained)
                .await
                .expect("retain the real dashboard stdout");
            retained
        });
        let mut lines = BufReader::new(
            server
                .stderr
                .take()
                .expect("capture the real dashboard stderr"),
        )
        .lines();
        let mut dashboard_stderr = String::new();
        let endpoint = timeout(Duration::from_secs(60), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .expect("read the real dashboard startup")
                    .expect("Stado dashboard exited before listening");
                eprintln!("DASHBOARD {line}");
                dashboard_stderr.push_str(&line);
                dashboard_stderr.push('\n');
                if let Some(endpoint) = line.strip_prefix("[dashboard] listening on ") {
                    return endpoint.to_string();
                }
            }
        })
        .await
        .expect("the real Stado dashboard must bind within 60 seconds");
        let remaining_logs = tokio::spawn(async move {
            let mut retained = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                retained.push_str(&line);
                retained.push('\n');
            }
            retained
        });
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(330))
            .build()
            .expect("construct the loopback dashboard client");

        let mut read_args = vec!["host", "exec", TARGET, "--json", "--"];
        read_args.extend_from_slice(self.story.words);
        let response = http
            .post(format!("{endpoint}/api/operator/run"))
            .header("X-Stado-Action", "operator-command")
            .json(&json!({
                "args": &read_args,
                "timeout_seconds": 300,
            }))
            .send()
            .await
            .expect("send the retained-log read to the real Stado dashboard");
        let read_status = response.status();
        let read_body = response
            .text()
            .await
            .expect("read the retained-log dashboard response");
        self.retain_http("dashboard-retained-log-read", read_status, &read_body);

        // The target is intentionally absent. A confirmation-gate regression
        // can therefore reach registry resolution, but can never reach the
        // host-side launcher or contact a provider.
        let mut sign_in_args = vec![
            "host",
            "exec",
            "provider-sign-in-must-never-resolve",
            "--json",
            "--",
        ];
        sign_in_args.extend_from_slice(PROVIDER_SIGN_IN);
        let response = http
            .post(format!("{endpoint}/api/operator/run"))
            .header("X-Stado-Action", "operator-command")
            .json(&json!({"args": &sign_in_args}))
            .send()
            .await
            .expect("send the unconfirmed provider sign-in to the real Stado dashboard");
        let refusal_status = response.status();
        let refusal_body = response
            .text()
            .await
            .expect("read the dashboard mutation refusal");
        self.retain_http(
            "dashboard-refuse-provider-sign-in",
            refusal_status,
            &refusal_body,
        );

        server
            .kill()
            .await
            .expect("stop only the isolated Stado dashboard");
        let server_status = server
            .wait()
            .await
            .expect("reap the isolated Stado dashboard");
        let dashboard_stdout = stdout_logs
            .await
            .expect("retain the dashboard stdout reader");
        dashboard_stderr.push_str(
            &remaining_logs
                .await
                .expect("retain the remaining dashboard stderr"),
        );
        write_private(&self.root.join("dashboard.stdout"), &dashboard_stdout);
        write_private(
            &self.root.join("dashboard.stderr"),
            dashboard_stderr.as_bytes(),
        );
        write_private(
            &self.root.join("dashboard-process.json"),
            &serde_json::to_vec_pretty(&json!({
                "schema": "stado.host-exec-retained-log-dashboard-process.v1",
                "binary": env!("CARGO_BIN_EXE_stado"),
                "args": ["dashboard", "--bind", "127.0.0.1", "--port", "0"],
                "exit_code": server_status.code(),
                "stopped_by_test": true,
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
            }))
            .unwrap(),
        );

        assert_eq!(
            read_status,
            reqwest::StatusCode::OK,
            "the exact retained-log read required mutation confirmation: {read_body}",
        );
        let read: Value = serde_json::from_str(&read_body)
            .expect("the real dashboard retained-log response is JSON");
        assert_eq!(read["read_only"], true, "{read:#}");
        assert_eq!(read["ok"], true, "{read:#}");
        assert_eq!(read["exit_code"], 0, "{read:#}");
        assert_eq!(
            read["stdout_truncated"], false,
            "Desktop cannot decode the real native log receipt after truncation; full response retained at {}",
            self.root.display(),
        );
        let receipt: Value = serde_json::from_str(
            read["stdout"]
                .as_str()
                .expect("Desktop receives the native command stdout"),
        )
        .expect("Desktop must be able to decode the complete native host-exec receipt");
        assert_eq!(receipt["target"], TARGET);
        assert_eq!(receipt["status"], "ok");
        assert_eq!(receipt["command"], self.story.words.join(" "));

        assert_eq!(
            refusal_status,
            reqwest::StatusCode::FORBIDDEN,
            "an unconfirmed provider sign-in crossed the dashboard mutation gate: {refusal_body}",
        );
        let refusal: Value = serde_json::from_str(&refusal_body)
            .expect("the real dashboard mutation refusal is JSON");
        assert_eq!(refusal["ok"], false, "{refusal:#}");
        assert_eq!(
            refusal["error"],
            "mutating commands require explicit RUN_MUTATION confirmation",
        );
    }
}

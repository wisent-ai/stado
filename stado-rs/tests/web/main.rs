//! Web-status diagnostics through the real CLI and native operator API.
//! Every declaration belongs to an isolated profile created by Stado itself.
//! The dashboard is the real product binary, and DNS uses the real resolver.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Journey {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Journey {
    fn new() -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/web-status");
        fs::create_dir_all(&evidence).unwrap();
        let root = tempfile::Builder::new()
            .prefix("web-status-")
            .tempdir_in(evidence)
            .unwrap()
            .keep();
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        let config = home.join(".stado/config.json");
        let journey = Self { root, home, config };
        journey.retain(
            "source.json",
            &json!({
                "source_revision": env!("STADO_SOURCE_REVISION"),
                "binary": env!("CARGO_BIN_EXE_stado"),
            }),
        );
        eprintln!("web-status evidence: {}", journey.root.display());
        journey
    }

    fn retain(&self, name: &str, value: &Value) {
        fs::write(
            self.root.join(name),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env(
                "WC_LOCAL_STORAGE_PATH",
                self.home.join(".stado/local-storage"),
            )
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1");
        command
    }

    fn run(&self, step: &str, args: &[&str]) -> Output {
        let output = self
            .command()
            .args(args)
            .output()
            .expect("start real Stado CLI");
        fs::write(self.root.join(format!("{step}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{step}.stderr")), &output.stderr).unwrap();
        self.retain(
            &format!("{step}.process.json"),
            &json!({
                "args": args,
                "exit_code": output.status.code(),
                "source_revision": env!("STADO_SOURCE_REVISION"),
            }),
        );
        output
    }

    fn success(&self, step: &str, args: &[&str]) -> Output {
        let output = self.run(step, args);
        assert!(
            output.status.success(),
            "{step}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    async fn operator(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
        step: &str,
        args: &[&str],
    ) -> (reqwest::StatusCode, Value) {
        let response = client
            .post(format!("{endpoint}/api/operator/run"))
            .header("X-Stado-Action", "operator-command")
            .json(&json!({"args": args}))
            .send()
            .await
            .expect("call the real native operator API");
        let status = response.status();
        let body = response.text().await.unwrap();
        fs::write(self.root.join(format!("{step}.body.json")), &body).unwrap();
        self.retain(
            &format!("{step}.http.json"),
            &json!({
                "endpoint": endpoint,
                "args": args,
                "http_status": status.as_u16(),
                "source_revision": env!("STADO_SOURCE_REVISION"),
            }),
        );
        (
            status,
            serde_json::from_str(&body).expect("operator response is JSON"),
        )
    }
}

fn assert_missing_edge(rows: &Value) {
    let rows = rows.as_array().expect("web status is an array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["product"], "release-probe");
    assert_eq!(rows[0]["verdict"], "edge-unconfigured");
    assert_eq!(
        rows[0]["edge_error"],
        "web_api.edge must be an object with target, address and contact"
    );
}

#[tokio::test]
async fn web_status_preserves_missing_edge_and_refusals_through_cli_and_api() {
    let journey = Journey::new();
    journey.success("initialize", &["config", "init"]);
    let empty = journey.success("empty", &["web", "status", "--json"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&empty.stdout).unwrap(),
        json!([])
    );
    journey.success(
        "declare",
        &[
            "web",
            "declare",
            "release-probe",
            "--hostname",
            "stado.wisent.com",
            "--redirect-to",
            "https://wisent.com",
            "--json",
        ],
    );
    let declared = fs::read(&journey.config).unwrap();
    let config: Value = serde_json::from_slice(&declared).unwrap();
    assert_eq!(
        config["web_api"]["products"]["release-probe"]["hostname"],
        "stado.wisent.com"
    );
    assert!(config["web_api"].get("edge").is_none());

    let cli = journey.run(
        "missing-edge-cli",
        &["web", "status", "release-probe", "--json"],
    );
    assert_eq!(cli.status.code(), Some(1));
    assert_missing_edge(&serde_json::from_slice(&cli.stdout).unwrap());
    assert_eq!(
        fs::read(&journey.config).unwrap(),
        declared,
        "status must not change the profile"
    );

    let mut server = tokio::process::Command::from(journey.command());
    server
        .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            fs::File::create(journey.root.join("dashboard.stdout")).unwrap(),
        ))
        .stderr(Stdio::from(
            fs::File::create(journey.root.join("dashboard.stderr")).unwrap(),
        ))
        .kill_on_drop(true);
    let mut server = server.spawn().expect("start the real headless Stado API");
    let deadline = Instant::now() + Duration::from_secs(60);
    let endpoint = loop {
        let log = fs::read_to_string(journey.root.join("dashboard.stderr")).unwrap();
        if let Some(endpoint) = log
            .lines()
            .find_map(|line| line.strip_prefix("[dashboard] listening on "))
        {
            break endpoint.to_owned();
        }
        assert!(
            server.try_wait().unwrap().is_none(),
            "dashboard exited: {log}"
        );
        assert!(
            Instant::now() < deadline,
            "dashboard did not become ready: {log}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let (read_status, read) = journey
        .operator(
            &client,
            &endpoint,
            "missing-edge-api",
            &["web", "status", "release-probe", "--json"],
        )
        .await;
    let (refusal_status, refusal) = journey
        .operator(
            &client,
            &endpoint,
            "unconfirmed-declaration",
            &[
                "web",
                "declare",
                "must-not-be-created",
                "--hostname",
                "stado.wisent.com",
                "--redirect-to",
                "https://wisent.com",
                "--json",
            ],
        )
        .await;
    server
        .kill()
        .await
        .expect("stop only the isolated test API");
    let server_status = server.wait().await.unwrap();
    journey.retain(
        "dashboard.process.json",
        &json!({
            "args": ["dashboard", "--bind", "127.0.0.1", "--port", "0"],
            "exit_code": server_status.code(),
            "stopped_by_test": true,
            "source_revision": env!("STADO_SOURCE_REVISION"),
        }),
    );

    assert_eq!(
        fs::read(&journey.config).unwrap(),
        declared,
        "neither API request may change the profile"
    );
    assert_eq!(read_status, reqwest::StatusCode::OK, "{read}");
    assert_eq!(read["read_only"], true);
    assert_eq!(read["ok"], false);
    assert_eq!(read["exit_code"], 1);
    assert_eq!(read["stdout_truncated"], false);
    assert_missing_edge(&serde_json::from_str(read["stdout"].as_str().unwrap()).unwrap());
    assert_eq!(refusal_status, reqwest::StatusCode::FORBIDDEN, "{refusal}");
    assert_eq!(
        refusal["error"],
        "mutating commands require explicit RUN_MUTATION confirmation"
    );

    journey.success(
        "remove-declaration",
        &["config", "unset", "web_api.products"],
    );
    let final_config: Value = serde_json::from_slice(&fs::read(&journey.config).unwrap()).unwrap();
    assert!(final_config["web_api"].get("products").is_none());
    let final_status = journey.success("empty-after-removal", &["web", "status", "--json"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&final_status.stdout).unwrap(),
        json!([])
    );
}

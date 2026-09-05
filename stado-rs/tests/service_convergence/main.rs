//! Real Services API convergence against the built Stado dashboard, a real
//! isolated Skarbiec vault, and this machine through Stado's same-host channel.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;
use skarbiec_support::{real_skarbiec_binary, SkarbiecFixture, SkarbiecItem};

const HOST: &str = "probierz-service-convergence-host";
const PATH_ENV: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

struct ClientGrant {
    name: String,
    actions: Vec<&'static str>,
    bearer: String,
}

impl ClientGrant {
    fn new(name: &str, actions: Vec<&'static str>, bearer: String) -> Self {
        Self {
            name: name.to_string(),
            actions,
            bearer,
        }
    }

    fn item(&self) -> String {
        format!("{}-registry-api", self.name)
    }
}

struct Answer {
    status: u16,
    body: Value,
}

struct DashboardFixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
    protected: PathBuf,
    stado: PathBuf,
    skarbiec: PathBuf,
    skarbiec_current: String,
    skarbiec_declared: String,
    address: SocketAddr,
    _vault: Option<SkarbiecFixture>,
    dashboard: Child,
}

impl DashboardFixture {
    fn start(clients: &[ClientGrant]) -> Self {
        let runs = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/service-convergence-runs");
        fs::create_dir_all(&runs).expect("service-convergence run root");
        let root = tempfile::Builder::new()
            .prefix("stado-service-convergence-")
            .tempdir_in(runs)
            .expect("repo-rooted service-convergence fixture");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let config = root.path().join("stado-config.json");
        let skarbiec_home = root.path().join("skarbiec-home");
        for directory in [
            &home,
            &storage,
            &skarbiec_home,
            &home.join("tmp"),
            &home.join(".stado/bin"),
            &home.join(".stado/protected"),
        ] {
            fs::create_dir_all(directory).expect("isolated fixture directory");
        }
        let port = unused_loopback_port();
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        fs::write(
            &config,
            serde_json::to_vec_pretty(&json!({
                "api": {"url": format!("http://{address}")},
                "storage": {"stado": {"url": format!("http://{address}")}}
            }))
            .expect("isolated Stado configuration JSON"),
        )
        .expect("isolated Stado configuration");

        let stado = home.join(".stado/bin/stado");
        let skarbiec = home.join(".stado/bin/skarbiec");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &stado).expect("copy built Stado binary");
        fs::copy(real_skarbiec_binary(), &skarbiec).expect("copy real Skarbiec binary");
        for binary in [&stado, &skarbiec] {
            fs::set_permissions(binary, fs::Permissions::from_mode(0o700))
                .expect("fixture binary is executable");
        }
        let skarbiec_current = binary_version(&skarbiec, &home);
        let skarbiec_declared = next_patch_version(&skarbiec_current);
        stage_current(&home, "stado", env!("CARGO_PKG_VERSION"), &stado);
        stage_current(&home, "skarbiec", &skarbiec_current, &skarbiec);
        let protected = home.join(".stado/protected/operator-state.json");
        fs::write(
            &protected,
            b"{\"owner\":\"probierz\",\"must_survive_failed_delivery\":true}\n",
        )
        .expect("protected fixture state");

        let hostname = hostname();
        let short_hostname = hostname.trim_end_matches(".local").to_string();
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": null,
                "release_platform": release_platform(),
                "hostnames": [hostname, short_hostname],
                "role": "interactive",
                "managed_versions": {
                    "skarbiec": skarbiec_declared,
                    "stado": env!("CARGO_PKG_VERSION")
                },
                "services": []
            }],
            "coordinators": []
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&registry).expect("registry JSON"),
        )
        .expect("isolated registry");

        let vault = if clients.is_empty() {
            None
        } else {
            let items = clients
                .iter()
                .map(|client| {
                    SkarbiecItem::new(
                        client.item(),
                        "token",
                        json!({
                            "schema": "skarbiec.item.v2",
                            "kind": "token",
                            "fields": {"token": client.bearer},
                            "context": {"service": "stado-registry-api", "client": client.name}
                        }),
                    )
                })
                .collect::<Vec<_>>();
            let capabilities = clients
                .iter()
                .map(|client| format!("read:{}#token", client.item()))
                .collect::<Vec<_>>()
                .join(",");
            Some(SkarbiecFixture::start(
                &skarbiec_home,
                &items,
                "stado-registry-api-verifier",
                &capabilities,
                "registry-api-verifier-grant",
            ))
        };

        let client_document = clients
            .iter()
            .map(|client| {
                (
                    client.name.clone(),
                    json!({"item": client.item(), "actions": client.actions}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let stdout =
            File::create(root.path().join("dashboard.stdout.log")).expect("dashboard stdout log");
        let stderr =
            File::create(root.path().join("dashboard.stderr.log")).expect("dashboard stderr log");
        let mut dashboard_command = Command::new(env!("CARGO_BIN_EXE_stado"));
        dashboard_command
            .args([
                "dashboard",
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", PATH_ENV)
            .env("TMPDIR", home.join("tmp"))
            .env("STADO_CONFIG", &config)
            .env("STADO_API_URL", format!("http://{address}"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "service-convergence")
            .env(
                "WC_REGISTRY_API_CLIENTS",
                Value::Object(client_document).to_string(),
            )
            .env("WC_DASHBOARD_BOUNDARY_ATTEMPTS", "1")
            .env("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS", "10")
            .env("WC_DASHBOARD_TRUST_HTTPS_PROXY", "true")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        if let Some(vault) = &vault {
            dashboard_command
                .env("WC_REGISTRY_SKARBIEC_URL", vault.url())
                .env(
                    "WC_REGISTRY_SKARBIEC_CONSUMER",
                    "stado-registry-api-verifier",
                )
                .env("WC_REGISTRY_SKARBIEC_TOKEN_FILE", &vault.token);
        }
        let dashboard = dashboard_command
            .spawn()
            .expect("built Stado dashboard starts");
        let mut fixture = Self {
            _root: root,
            home,
            storage,
            config,
            protected,
            stado,
            skarbiec,
            skarbiec_current,
            skarbiec_declared,
            address,
            _vault: vault,
            dashboard,
        };
        fixture.wait_until_ready();
        fixture
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if TcpStream::connect_timeout(&self.address, Duration::from_millis(200)).is_ok()
                && self
                    .request("GET", "/api/service/converge?target=readiness", None, "")
                    .status
                    == 401
            {
                return;
            }
            if let Some(status) = self.dashboard.try_wait().expect("read dashboard status") {
                panic!(
                    "dashboard exited before readiness with {status}: {}",
                    fs::read_to_string(self._root.path().join("dashboard.stderr.log"))
                        .unwrap_or_default()
                );
            }
            assert!(
                Instant::now() < deadline,
                "dashboard never listened at {}: {}",
                self.address,
                fs::read_to_string(self._root.path().join("dashboard.stderr.log"))
                    .unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn request(&self, method: &str, target: &str, bearer: Option<&str>, body: &str) -> Answer {
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(5))
            .expect("connect to real dashboard");
        stream
            .set_read_timeout(Some(Duration::from_secs(180)))
            .expect("dashboard read timeout");
        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: stado.wisent.com\r\nX-Forwarded-Proto: https\r\nX-Forwarded-For: 203.0.113.10\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(bearer) = bearer {
            request.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream
            .write_all(request.as_bytes())
            .expect("write dashboard request");
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .expect("dashboard answers and closes");
        let raw = String::from_utf8(raw).expect("dashboard answer is UTF-8");
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .expect("dashboard answer has a head and body");
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .expect("dashboard answer has an HTTP status");
        Answer {
            status,
            body: serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("dashboard body is not JSON ({error}): {body}")),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }
}

fn binary_version(binary: &Path, home: &Path) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .env_clear()
        .env("HOME", home)
        .env("PATH", PATH_ENV)
        .output()
        .expect("real binary version probe runs");
    assert!(
        output.status.success(),
        "real binary version probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("real binary version is UTF-8")
        .split_whitespace()
        .find(|candidate| {
            let parts = candidate.split('.').collect::<Vec<_>>();
            parts.len() == 3 && parts.iter().all(|part| part.parse::<u64>().is_ok())
        })
        .expect("real binary reports an exact semantic version")
        .to_string()
}

fn next_patch_version(version: &str) -> String {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u64>().expect("semantic version component"))
        .collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        3,
        "fixture version must be exact semantic version"
    );
    parts[2] += 1;
    format!("{}.{}.{}", parts[0], parts[1], parts[2])
}

impl Drop for DashboardFixture {
    fn drop(&mut self) {
        let _ = self.dashboard.kill();
        let _ = self.dashboard.wait();
    }
}

fn stage_current(home: &Path, name: &str, version: &str, binary: &Path) {
    let coordinate = home
        .join(".stado/releases")
        .join(name)
        .join(version)
        .join(release_platform());
    fs::create_dir_all(&coordinate).expect("staged release coordinate");
    fs::copy(binary, coordinate.join(name)).expect("stage delivered binary bytes");
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no convergence platform mapping for {os}-{arch}"),
    }
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("hostname command runs");
    assert!(output.status.success(), "hostname command succeeds");
    String::from_utf8(output.stdout)
        .expect("hostname is UTF-8")
        .trim()
        .to_string()
}

fn unused_loopback_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved listener address")
        .port()
}

fn skarbiec_generated_bearer() -> String {
    let output = Command::new(real_skarbiec_binary())
        .args([
            "generate", "--length", "64", "--lower", "--upper", "--digits",
        ])
        .env_clear()
        .env("HOME", std::env::var_os("HOME").expect("HOME is set"))
        .env("PATH", PATH_ENV)
        .output()
        .expect("real Skarbiec generates registry API bearer");
    assert!(
        output.status.success(),
        "real Skarbiec bearer generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated: Value =
        serde_json::from_slice(&output.stdout).expect("Skarbiec generation response is JSON");
    let bearer = generated["password"]
        .as_str()
        .expect("Skarbiec generation response carries password")
        .to_string();
    assert_eq!(bearer.len(), 64, "Skarbiec generated the requested bearer");
    bearer
}

fn digest(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).expect("read protected file")))
}

fn binary_rows(report: &Value) -> BTreeMap<&str, &Value> {
    report["binaries"]
        .as_array()
        .expect("report carries binaries")
        .iter()
        .map(|row| (row["binary"].as_str().expect("row names its binary"), row))
        .collect()
}

fn assert_error(answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "unexpected response: {}",
        answer.body
    );
    assert_eq!(answer.body["ok"], false, "error envelope: {}", answer.body);
    assert_eq!(
        answer.body["error"]["code"], code,
        "error envelope: {}",
        answer.body
    );
    assert!(
        answer.body["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "error envelope has no diagnosis: {}",
        answer.body
    );
}

#[test]
#[ignore = "Probierz owns the real Skarbiec/dashboard/host qualification"]
fn authenticated_services_api_converges_real_same_host_state() {
    let read_bearer = skarbiec_generated_bearer();
    let apply_bearer = skarbiec_generated_bearer();
    let clients = [
        ClientGrant::new("read-only", vec!["converge-read"], read_bearer.clone()),
        ClientGrant::new("apply-only", vec!["converge-apply"], apply_bearer.clone()),
    ];
    let fixture = DashboardFixture::start(&clients);
    let protected_baseline = digest(&fixture.protected);
    let stado_baseline = digest(&fixture.stado);
    let skarbiec_baseline = digest(&fixture.skarbiec);
    let selected = format!("/api/service/converge?target={HOST}&binary=stado");
    let host_wide = format!("/api/service/converge?target={HOST}");

    let no_bearer = fixture.request("GET", &selected, None, "");
    assert_eq!(no_bearer.status, 401, "missing bearer: {}", no_bearer.body);
    assert_eq!(no_bearer.body, json!({"error": "unauthorized"}));
    let wrong_bearer = fixture.request("GET", &selected, Some("not-a-real-grant"), "");
    assert_eq!(
        wrong_bearer.status, 401,
        "wrong bearer: {}",
        wrong_bearer.body
    );
    assert_eq!(wrong_bearer.body, json!({"error": "unauthorized"}));

    let apply_cannot_read = fixture.request("GET", &selected, Some(&apply_bearer), "");
    assert_eq!(
        apply_cannot_read.status, 401,
        "apply-only grant read the route: {}",
        apply_cannot_read.body
    );
    let read_cannot_apply = fixture.request("POST", &selected, Some(&read_bearer), "");
    assert_eq!(
        read_cannot_apply.status, 401,
        "read-only grant applied convergence: {}",
        read_cannot_apply.body
    );

    for malformed in [
        "/api/service/converge",
        "/api/service/converge?binary=stado",
        "/api/service/converge?target=",
        "/api/service/converge?target=one&binary=",
        "/api/service/converge?target=one&target=two",
        "/api/service/converge?target=one&binary=stado&binary=skarbiec",
        "/api/service/converge?target=one&unexpected=value",
        "/api/service/converge?target=%GG",
        "/api/service/converge?target=%FF",
    ] {
        assert_error(
            &fixture.request("GET", malformed, Some(&read_bearer), ""),
            400,
            "INVALID_REQUEST",
        );
    }
    assert_error(
        &fixture.request("POST", &selected, Some(&apply_bearer), "{}"),
        400,
        "INVALID_REQUEST",
    );
    assert_error(
        &fixture.request(
            "GET",
            "/api/service/converge?target=no-such-declared-host",
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_error(
        &fixture.request(
            "GET",
            &format!("/api/service/converge?target={HOST}&binary=no-such-binary"),
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);

    let current = fixture.request("GET", &selected, Some(&read_bearer), "");
    assert_eq!(current.status, 200, "selected GET: {}", current.body);
    assert_eq!(
        current.body["exit_code"], 0,
        "selected GET: {}",
        current.body
    );
    assert_eq!(current.body["report"]["target"], HOST);
    assert_eq!(current.body["report"]["applied"], false);
    let selected_rows = current.body["report"]["binaries"]
        .as_array()
        .expect("selected report carries rows");
    assert_eq!(selected_rows.len(), 1, "selected report: {}", current.body);
    assert_eq!(selected_rows[0]["binary"], "stado");
    assert_eq!(selected_rows[0]["verdict"], "in-sync");
    assert_eq!(
        selected_rows[0]["declared_version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        selected_rows[0]["installed_version"],
        env!("CARGO_PKG_VERSION")
    );

    let applied_current = fixture.request("POST", &selected, Some(&apply_bearer), "");
    assert_eq!(
        applied_current.status, 200,
        "selected POST: {}",
        applied_current.body
    );
    assert_eq!(
        applied_current.body["exit_code"], 0,
        "selected POST: {}",
        applied_current.body
    );
    assert_eq!(applied_current.body["report"]["applied"], true);

    let report = fixture.request("GET", &host_wide, Some(&read_bearer), "");
    assert_eq!(report.status, 200, "host-wide GET: {}", report.body);
    assert_ne!(
        report.body["exit_code"], 0,
        "host-wide GET: {}",
        report.body
    );
    let rows = binary_rows(&report.body["report"]);
    assert_eq!(rows.len(), 2, "host-wide GET: {}", report.body);
    assert_eq!(rows["stado"]["verdict"], "in-sync");
    assert_eq!(rows["skarbiec"]["verdict"], "host-behind");
    assert_eq!(
        rows["skarbiec"]["installed_version"],
        fixture.skarbiec_current
    );
    assert_eq!(
        rows["skarbiec"]["declared_version"],
        fixture.skarbiec_declared
    );

    let failed = fixture.request("POST", &host_wide, Some(&apply_bearer), "");
    assert_eq!(failed.status, 200, "failed apply envelope: {}", failed.body);
    assert_ne!(failed.body["exit_code"], 0, "failed apply: {}", failed.body);
    assert_eq!(failed.body["report"]["applied"], true);
    let releases = failed.body["report"]["releases"]
        .as_array()
        .expect("failed apply retains releases");
    let release = releases
        .iter()
        .find(|release| release["binary"] == "skarbiec")
        .expect("failed Skarbiec delivery remains in the report");
    assert_eq!(release["version"], fixture.skarbiec_declared);
    assert_eq!(release["status"], "failed");
    println!("failed convergence receipt: {}", failed.body);
    let final_rows = binary_rows(&failed.body["report"]);
    assert_eq!(final_rows["stado"]["verdict"], "in-sync");
    assert_eq!(final_rows["skarbiec"]["verdict"], "host-behind");
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);

    println!(
        "verified authenticated nonlocal Services API on {}: selected-current=stado {}; failed-delivery=skarbiec {}->{}; exit={}",
        release_platform(),
        env!("CARGO_PKG_VERSION"),
        fixture.skarbiec_current,
        fixture.skarbiec_declared,
        failed.body["exit_code"]
    );
}

#[test]
#[ignore = "Probierz Desktop CUA owns this long-lived real fixture"]
fn service_convergence_cua_fixture() {
    // This fixture performs no Wisent account operation. The desktop receives
    // only an owner-readable local token file for a dedicated registry API
    // client whose bearer is resolved through the real isolated Skarbiec.
    let ready = PathBuf::from(
        std::env::var_os("STADO_SERVICE_CONVERGENCE_READY")
            .expect("STADO_SERVICE_CONVERGENCE_READY is required"),
    );
    let stop = PathBuf::from(
        std::env::var_os("STADO_SERVICE_CONVERGENCE_STOP")
            .expect("STADO_SERVICE_CONVERGENCE_STOP is required"),
    );
    let bearer = skarbiec_generated_bearer();
    let clients = [ClientGrant::new(
        "desktop-local",
        vec!["policy-read", "converge-read", "converge-apply"],
        bearer.clone(),
    )];
    let mut fixture = DashboardFixture::start(&clients);
    let token_file = fixture.home.join(".stado/desktop-registry-api-token");
    fs::write(&token_file, bearer).expect("write desktop registry API bearer");
    fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600))
        .expect("desktop registry API bearer is owner-readable only");
    let stado_baseline = digest(&fixture.stado);
    let skarbiec_baseline = digest(&fixture.skarbiec);
    let registry_baseline = digest(&fixture.storage.join("registry.json"));
    let readiness = json!({
        "endpoint": fixture.endpoint(),
        "home": fixture.home,
        "storage": fixture.storage,
        "config": fixture.config,
        "binary": env!("CARGO_BIN_EXE_stado"),
        "target": HOST,
        "token_file": token_file
    });
    fs::write(
        &ready,
        serde_json::to_vec_pretty(&readiness).expect("readiness JSON"),
    )
    .expect("write CUA fixture readiness");
    println!(
        "service-convergence CUA fixture ready at {}",
        fixture.endpoint()
    );

    while !stop.exists() {
        if let Some(status) = fixture.dashboard.try_wait().expect("read dashboard status") {
            panic!("dashboard exited during CUA journey with {status}");
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);
    assert_eq!(
        digest(&fixture.storage.join("registry.json")),
        registry_baseline
    );
}

//! The real CLI and Desktop API retain the operating system's request failure.
//! TCP port zero cannot host a listener: bind(0) selects a nonzero port. The
//! kernel rejects the real connection without a replacement server or response.

use std::fs::{self, File};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

struct Journey {
    root: PathBuf,
    binary: PathBuf,
    origin: String,
    cause: String,
}

impl Journey {
    fn new(name: &str) -> Self {
        let evidence =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/http-diagnostics");
        fs::create_dir_all(&evidence).unwrap();
        let root = tempfile::Builder::new()
            .prefix(&format!("{name}-"))
            .tempdir_in(evidence)
            .unwrap()
            .keep();
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let failure = TcpStream::connect(address).unwrap_err();
        let origin = format!("http://{address}");
        let bearer = root.join("bearer");
        fs::write(&bearer, "diagnostic-only\n").unwrap();
        fs::set_permissions(&bearer, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(
            root.join("config.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "storage": { "backend": "stado", "stado": {
                    "url": origin, "namespace": "diagnostics", "token_file": bearer
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        let binary = std::env::var_os("STADO_DIAGNOSTIC_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_stado")));
        let journey = Self {
            root,
            binary,
            origin,
            cause: failure.to_string(),
        };
        let version = journey.run(&["--version"], "version");
        assert!(version.status.success());
        eprintln!("diagnostic evidence: {}", journey.root.display());
        journey
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.root)
            .env("TMPDIR", &self.root)
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env("NO_COLOR", "1")
            .env("NO_PROXY", "*")
            .env("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS", "1");
        command
    }

    fn run(&self, args: &[&str], step: &str) -> Output {
        let output = self.command().args(args).output().unwrap();
        fs::write(self.root.join(format!("{step}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{step}.stderr")), &output.stderr).unwrap();
        fs::write(
            self.root.join(format!("{step}.json")),
            serde_json::to_vec_pretty(&json!({
                "binary": self.binary, "args": args, "exit_code": output.status.code(),
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
                "observed_os_error": self.cause, "origin": self.origin
            }))
            .unwrap(),
        )
        .unwrap();
        output
    }

    fn assert_cause(&self, message: &str) {
        assert!(
            message.contains(&self.origin),
            "failed endpoint missing: {message}"
        );
        assert!(
            message.contains("error sending request"),
            "failed operation missing: {message}"
        );
        assert!(
            message.contains(&self.cause),
            "actual OS cause was discarded: {message}"
        );
    }
}

#[test]
fn storage_read_names_the_operating_system_failure() {
    let journey = Journey::new("storage-read");
    let output = journey.run(&["storage", "cat", "system/storage-layout.json"], "cat");
    assert_eq!(output.status.code(), Some(69));
    assert!(output.stdout.is_empty());
    journey.assert_cause(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn release_submit_keeps_the_cause_of_its_failed_registry_read() {
    let journey = Journey::new("release-submit");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = journey.run(
        &[
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--channel",
            "candidate",
            "--json",
        ],
        "submit",
    );
    assert_eq!(
        output.status.code(),
        Some(69),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    journey.assert_cause(&String::from_utf8_lossy(&output.stderr));
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn desktop_command_api_preserves_the_real_storage_failure() {
    let journey = Journey::new("desktop-api");
    let server_log = journey.root.join("dashboard.stderr");
    let mut server = Server(
        journey
            .command()
            .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", journey.root.join("api-storage"))
            .stdin(Stdio::null())
            .stdout(File::create(journey.root.join("dashboard.stdout")).unwrap())
            .stderr(File::create(&server_log).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let endpoint = loop {
        let log = fs::read_to_string(&server_log).unwrap();
        if let Some(origin) = log
            .lines()
            .find_map(|line| line.strip_prefix("[dashboard] listening on "))
        {
            break format!("{origin}/api/operator/run");
        }
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "dashboard exited: {log}"
        );
        assert!(Instant::now() < deadline, "dashboard did not start: {log}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let request = json!({
        "args": ["storage", "cat", "system/storage-layout.json"],
        "timeout_seconds": 20
    });
    fs::write(
        journey.root.join("operator.request.json"),
        serde_json::to_vec_pretty(&request).unwrap(),
    )
    .unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(25))
        .build()
        .unwrap()
        .post(&endpoint)
        .header("X-Stado-Action", "operator-command")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.bytes().await.unwrap();
    fs::write(journey.root.join("operator.response.json"), &body).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(report["exit_code"], 69);
    assert_eq!(report["stderr_truncated"], false);
    journey.assert_cause(report["stderr"].as_str().unwrap());
    let layout: serde_json::Value = serde_json::from_slice(
        &fs::read(journey.root.join("api-storage/system/storage-layout.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(layout["product"], "stado");
}

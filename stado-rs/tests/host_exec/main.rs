//! Real current-host journey for the fixed retained-Tailscale-log read.
//!
//! The ignored journey runs the built Stado CLI against an isolated local
//! registry naming the machine executing the test. The production host channel
//! therefore takes its current-host path and executes the operating system's
//! real logging tool: `/usr/bin/log` on macOS or `/usr/bin/journalctl` on Linux.
//! No executable is substituted and no SSH destination exists in the fixture.
//! The same story then sends the fixed read through a real Stado dashboard
//! without mutation confirmation and verifies that an exact provider sign-in
//! remains behind `RUN_MUTATION`; its deliberately unknown target makes even a
//! confirmation-classification regression stop before any provider contact.
//!
//! Process stdout, stderr, arguments, and exit status are retained under the
//! repository's ignored `.wisent-output/host-exec-retained-logs` directory.
//! Empty native output is recorded as empty; it is not treated as proof about
//! Funnel. A missing logging executable or a failed native read fails this
//! journey instead of being skipped or replaced.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::timeout;

use serde_json::{json, Value};

const TARGET: &str = "retained-log-current-host";
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

const PROVIDER_SIGN_IN: &[&str] = &[
    "start-with-skarbiec",
    "subscription",
    "sign-in",
    "codex",
    "--login-item",
    "codex-wisent-google-sso",
    "--reason",
    "codex-grant-disowned-2026-08-27-gateway-has-one-live-provider",
    "--login-timeout-ms",
    "900000",
    "--json",
];

const MACOS_LOG_WORDS: &[&str] = &[
    "log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
const MACOS_LOG_ARGV: &[&str] = &[
    "/usr/bin/log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
const MACOS_WIDER_TIME: &[&str] = &[
    "log",
    "show",
    "--last",
    "2h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
const MACOS_EXTRA_PROCESS: &[&str] = &[
    "log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
    "--process",
    "stado-retained-log-probierz-never-runs",
];
// `log config` is the modifying verb. The deliberately invalid mode keeps the
// journey harmless even if a future regression accidentally executes it.
const MACOS_MODIFYING_LOG: &[&str] = &["log", "config", "--mode", "definitely-not-a-log-mode"];

const LINUX_LOG_WORDS: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
];
const LINUX_LOG_ARGV: &[&str] = &[
    "/usr/bin/journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
];
const LINUX_WIDER_TIME: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-2h",
    "--no-pager",
    "--output",
    "short-iso",
];
const LINUX_EXTRA_UNIT: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
    "--unit",
    "stado-retained-log-probierz-never-runs.service",
];
// Vacuuming is a modifying journal operation. Its invalid duration ensures the
// native tool could not vacuum anything even if this refusal ever regressed.
const LINUX_MODIFYING_LOG: &[&str] = &["journalctl", "--vacuum-time", "definitely-not-a-duration"];

struct NativeStory {
    platform: &'static str,
    program: &'static str,
    words: &'static [&'static str],
    argv: &'static [&'static str],
    wider_time: &'static [&'static str],
    wider_source: &'static [&'static str],
    modifying: &'static [&'static str],
}

fn native_story() -> NativeStory {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => NativeStory {
            platform: "darwin-arm64",
            program: "/usr/bin/log",
            words: MACOS_LOG_WORDS,
            argv: MACOS_LOG_ARGV,
            wider_time: MACOS_WIDER_TIME,
            wider_source: MACOS_EXTRA_PROCESS,
            modifying: MACOS_MODIFYING_LOG,
        },
        ("linux", "x86_64") => NativeStory {
            platform: "linux-amd64",
            program: "/usr/bin/journalctl",
            words: LINUX_LOG_WORDS,
            argv: LINUX_LOG_ARGV,
            wider_time: LINUX_WIDER_TIME,
            wider_source: LINUX_EXTRA_UNIT,
            modifying: LINUX_MODIFYING_LOG,
        },
        (os, arch) => panic!(
            "blocked: retained-log host-exec journey requires macOS arm64 or Linux amd64, got {os}-{arch}"
        ),
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| panic!("create retained evidence {}: {error}", path.display()));
    file.write_all(bytes)
        .unwrap_or_else(|error| panic!("write retained evidence {}: {error}", path.display()));
}

fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(
        output.status.success(),
        "blocked: the real hostname executable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let hostname = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_string();
    assert!(
        !hostname.is_empty(),
        "the real current host has no hostname"
    );
    hostname
}

struct Journey {
    root: PathBuf,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
    registry: PathBuf,
    hostname: String,
    story: NativeStory,
    config_before: Vec<u8>,
    registry_before: Vec<u8>,
}

impl Journey {
    fn new() -> Self {
        let story = native_story();
        let native = fs::metadata(story.program).unwrap_or_else(|error| {
            panic!(
                "blocked: required native logging executable {} is unavailable: {error}",
                story.program
            )
        });
        assert!(
            native.is_file() && native.permissions().mode() & 0o111 != 0,
            "blocked: required native logging dependency {} is not an executable file",
            story.program,
        );

        let evidence =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/host-exec-retained-logs");
        fs::create_dir_all(&evidence).expect("create repository retained-evidence root");
        let root = tempfile::Builder::new()
            .prefix("retained-log-")
            .tempdir_in(&evidence)
            .expect("create repository-rooted retained-log journey")
            .keep();
        let home = root.join("home");
        let storage = root.join("storage");
        let tmp = root.join("tmp");
        for directory in [&home, &storage, &tmp] {
            fs::create_dir_all(directory).expect("create isolated journey directory");
        }
        let config = root.join("config.json");
        write_private(
            &config,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "storage": {
                    "backend": "local",
                    "local": {"path": storage},
                },
            }))
            .unwrap(),
        );

        let hostname = hostname();
        let registry = storage.join("registry.json");
        write_private(
            &registry,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": 2,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": story.platform,
                    "hostnames": [hostname],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        );

        let config_before = fs::read(&config).expect("read isolated config baseline");
        let registry_before = fs::read(&registry).expect("read isolated registry baseline");
        let journey = Self {
            root,
            home,
            storage,
            config,
            registry,
            hostname,
            story,
            config_before,
            registry_before,
        };
        assert!(journey.home.starts_with(&journey.root));
        assert!(journey.config.starts_with(&journey.root));
        assert!(journey.registry.starts_with(&journey.root));
        eprintln!("retained-log evidence: {}", journey.root.display());
        journey
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "host-exec-retained-logs")
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1");
        command
    }

    fn invoke(&self, step: &str, words: &[&str]) -> Output {
        let mut args = vec!["host", "exec", TARGET, "--json", "--"];
        args.extend_from_slice(words);
        let output = self
            .command()
            .args(&args)
            .output()
            .unwrap_or_else(|error| panic!("the built Stado binary did not start: {error}"));
        write_private(&self.root.join(format!("{step}.stdout")), &output.stdout);
        write_private(&self.root.join(format!("{step}.stderr")), &output.stderr);
        write_private(
            &self.root.join(format!("{step}.json")),
            &serde_json::to_vec_pretty(&json!({
                "schema": "stado.host-exec-retained-log-process.v1",
                "binary": env!("CARGO_BIN_EXE_stado"),
                "args": args,
                "exit_code": output.status.code(),
                "success": output.status.success(),
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
                "host": self.hostname,
                "platform": self.story.platform,
                "native_program": self.story.program,
            }))
            .unwrap(),
        );
        output
    }

    fn retain_http(&self, step: &str, status: reqwest::StatusCode, body: &str) {
        write_private(
            &self.root.join(format!("{step}.body.json")),
            body.as_bytes(),
        );
        write_private(
            &self.root.join(format!("{step}.http.json")),
            &serde_json::to_vec_pretty(&json!({
                "schema": "stado.host-exec-retained-log-http.v1",
                "http_status": status.as_u16(),
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
            }))
            .unwrap(),
        );
    }

    async fn verify_dashboard_boundary(&self) {
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
        if read["structured"].is_null() {
            assert_eq!(
                read["stdout_truncated"], true,
                "a complete successful retained-log receipt must be structured: {read:#}",
            );
            assert!(
                !read["stdout"].as_str().unwrap_or_default().is_empty(),
                "a truncated native receipt must retain its bounded prefix: {read:#}",
            );
        } else {
            assert_eq!(read["structured"]["target"], TARGET, "{read:#}");
            assert_eq!(read["structured"]["status"], "ok", "{read:#}");
            assert_eq!(
                read["structured"]["command"],
                self.story.words.join(" "),
                "{read:#}",
            );
        }

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

    fn assert_unchanged(&self) {
        assert_eq!(
            fs::read(&self.config).expect("read isolated config after journey"),
            self.config_before,
            "the read-only journey changed its isolated Stado config",
        );
        assert_eq!(
            fs::read(&self.registry).expect("read isolated registry after journey"),
            self.registry_before,
            "the read-only journey changed its isolated registry",
        );
    }
}

fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn json_stdout(output: &Output, operation: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{operation} did not return one JSON document ({error}):\n{}",
            said(output)
        )
    })
}

fn assert_native_read(journey: &Journey, output: &Output) -> (usize, usize) {
    assert!(
        output.status.success(),
        "the real native retained-log read failed; its unmodified process output is retained:\n{}",
        said(output),
    );
    let report = json_stdout(output, "retained-log read");
    assert_eq!(report["schema"], "stado.host-exec-receipt.v1");
    assert_eq!(report["target"], TARGET);
    assert_eq!(report["ssh"], Value::Null);
    assert_eq!(report["ssh_fallbacks"], json!([]));
    assert_eq!(report["command"], journey.story.words.join(" "));
    assert_eq!(report["argv"], json!(journey.story.argv));
    assert_eq!(report["resolved_executable"], journey.story.program);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["exit_code"], 0);
    assert_eq!(report["error"], Value::Null);

    let stdout = report["stdout"]
        .as_str()
        .expect("native stdout is present verbatim in the receipt");
    let stderr = report["stderr"]
        .as_str()
        .expect("native stderr is present verbatim in the receipt");
    write_private(&journey.root.join("native.stdout"), stdout.as_bytes());
    write_private(&journey.root.join("native.stderr"), stderr.as_bytes());
    (stdout.len(), stderr.len())
}

fn assert_refused(output: &Output, words: &[&str], canonical: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "an attempt to widen or modify the native read was not refused with the policy exit:\n{}",
        said(output),
    );
    let report = json_stdout(output, "host-exec refusal");
    let requested = words.join(" ");
    assert_eq!(report["status"], "error");
    assert_eq!(report["failure_point"], "cli.host.exec");
    assert_eq!(report["error_code"], "refused");
    assert_eq!(report["retryable"], false);
    assert_eq!(
        report["message"],
        format!("'{requested}' is not an approved host-exec command"),
    );
    let help = report["help"]
        .as_str()
        .expect("a refusal carries the approved fixed spellings as separate help");
    assert!(
        help.starts_with("approved commands: ") && help.contains(canonical),
        "refusal help omitted the fixed retained-log read: {help}",
    );
}

#[tokio::test]
#[ignore = "requires and records the current host's real retained logging service"]
async fn retained_tailscale_logs_are_a_fixed_native_read_and_cannot_be_widened() {
    let journey = Journey::new();

    // Execute the whole story before judging it so every real process result is
    // retained even if one later assertion identifies a regression.
    let retained = journey.invoke("retained-log-read", journey.story.words);
    let wider_time = journey.invoke("refuse-wider-time", journey.story.wider_time);
    let wider_source = journey.invoke("refuse-extra-process-or-unit", journey.story.wider_source);
    let modifying = journey.invoke("refuse-modifying-log-operation", journey.story.modifying);
    journey.verify_dashboard_boundary().await;

    let (stdout_bytes, stderr_bytes) = assert_native_read(&journey, &retained);
    let canonical = journey.story.words.join(" ");
    assert_refused(&wider_time, journey.story.wider_time, &canonical);
    assert_refused(&wider_source, journey.story.wider_source, &canonical);
    assert_refused(&modifying, journey.story.modifying, &canonical);
    journey.assert_unchanged();

    write_private(
        &journey.root.join("journey.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema": "stado.host-exec-retained-log-journey.v1",
            "status": "completed",
            "test_source_revision": env!("STADO_SOURCE_REVISION"),
            "host": journey.hostname,
            "platform": journey.story.platform,
            "native_program": journey.story.program,
            "native_exit_code": retained.status.code(),
            "native_stdout_bytes": stdout_bytes,
            "native_stderr_bytes": stderr_bytes,
            "native_output_empty": stdout_bytes == 0 && stderr_bytes == 0,
            "refusals": 3,
            "config_unchanged": true,
            "registry_unchanged": true,
            "dashboard_read_without_confirmation": true,
            "dashboard_provider_sign_in_without_confirmation": "refused",
            "funnel_verdict": "not asserted",
        }))
        .unwrap(),
    );
    println!(
        "retained-log read verified: platform={}; native_exit=0; stdout_bytes={stdout_bytes}; stderr_bytes={stderr_bytes}; refusals=3; dashboard_read_only=true; dashboard_sign_in_refused=true; funnel=not-asserted; evidence={}",
        journey.story.platform,
        journey.root.display(),
    );
}

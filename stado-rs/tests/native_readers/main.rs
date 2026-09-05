//! Native-reader convergence against the init system that owns the process.
//!
//! This is deliberately a Probierz-only macOS story. It loads a uniquely named
//! LaunchAgent into the current login's real launchd domain and serves the real
//! `stado dashboard` on an isolated loopback port. The unit starts from a
//! private copy of the built Stado binary. While that process is still live, the
//! plist is changed to name the delivered `$HOME/.stado/bin/stado`, reproducing
//! the state in which launchd retains an old cached definition while the file on
//! disk already carries the new one.
//!
//! The regression in 0.16.21 matched only the running image's pathname against
//! the delivered root. It therefore skipped this unit: the process mapped the
//! private path while the on-disk declaration named the root. Convergence must
//! reload the changed definition through the same observed launchd domain,
//! prove the replacement maps the delivered inode, and leave that replacement
//! alone on a repeated convergence.

#![cfg(target_os = "macos")]

use sha2::Digest;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HOST: &str = "probierz-native-readers-host";
const PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
    label: String,
    domain: String,
    plist: PathBuf,
    root_binary: PathBuf,
    private_binary: PathBuf,
    archive: PathBuf,
    archive_sha256: String,
    port: u16,
    cleanup_finished: bool,
}

impl Fixture {
    fn new() -> Self {
        let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/native-reader-runs");
        fs::create_dir_all(&run_root).expect("native-reader run root");
        let root = tempfile::Builder::new()
            .prefix("stado-native-readers-")
            .tempdir_in(run_root)
            .expect("repo-rooted native-reader fixture");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let config = root.path().join("stado-config.json");
        let agents = home.join("Library/LaunchAgents");
        let delivered = home.join(".stado/bin");
        let private = home.join(".stado/native-readers/private");
        for directory in [
            &home,
            &storage,
            &agents,
            &delivered,
            &private,
            &home.join("tmp"),
        ] {
            fs::create_dir_all(directory).expect("isolated fixture directory");
        }
        fs::write(&config, b"{}\n").expect("isolated Stado configuration");

        let root_binary = delivered.join("stado");
        let private_binary = private.join("stado");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &root_binary)
            .expect("copy built Stado into the delivered root");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &private_binary)
            .expect("copy built Stado into the private root");

        let archive = root.path().join("stado-readers.tar.gz");
        let compressed = flate2::write::GzEncoder::new(
            fs::File::create(&archive).expect("create real Stado archive"),
            flate2::Compression::fast(),
        );
        let mut package = tar::Builder::new(compressed);
        package
            .append_path_with_name(&root_binary, "stado")
            .expect("archive the built Stado executable");
        package
            .into_inner()
            .expect("finish native archive")
            .finish()
            .expect("finish native archive compression");
        let archive_sha256 = file_identity(&archive).sha256;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after Unix epoch")
            .as_nanos();
        let label = format!(
            "com.wisent.probierz.native-readers.{}.{}",
            std::process::id(),
            unique
        );
        let domain = available_login_domain();
        let plist = agents.join(format!("{label}.plist"));
        let port = unused_loopback_port();

        let fixture = Self {
            _root: root,
            home,
            storage,
            config,
            label,
            domain,
            plist,
            root_binary,
            private_binary,
            archive,
            archive_sha256,
            port,
            cleanup_finished: false,
        };
        fixture.write_registry();
        fixture.write_plist(&fixture.private_binary);
        fixture
    }

    fn write_registry(&self) {
        let hostname = hostname();
        let short_hostname = hostname.trim_end_matches(".local").to_string();
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": null,
                "release_platform": "darwin-arm64",
                "hostnames": [hostname, short_hostname],
                "role": "interactive",
                "managed_versions": {},
                "services": [{
                    "name": self.label,
                    "unit": "",
                    "label": self.label,
                    "path": self.plist,
                    "kind": "launchd",
                    "managed_since": "2026-09-05T00:00:00Z"
                }]
            }],
            "coordinators": []
        });
        fs::write(
            self.storage.join("registry.json"),
            serde_json::to_vec_pretty(&document).expect("fixture registry JSON"),
        )
        .expect("isolated fixture registry");
    }

    fn write_plist(&self, program: &Path) {
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>dashboard</string>
    <string>--enrollment-only</string>
    <string>--bind</string>
    <string>127.0.0.1</string>
    <string>--port</string>
    <string>{port}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>{home}</string>
    <key>PATH</key><string>{path}</string>
    <key>TMPDIR</key><string>{tmp}</string>
    <key>WC_STORAGE_BACKEND</key><string>local</string>
    <key>WC_LOCAL_STORAGE_PATH</key><string>{storage}</string>
    <key>WC_STADO_STORAGE_NAMESPACE</key><string>probierz-native-readers</string>
    <key>STADO_CONFIG</key><string>{config}</string>
  </dict>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict>
</plist>
"#,
            label = xml(&self.label),
            program = xml(&program.to_string_lossy()),
            port = self.port,
            home = xml(&self.home.to_string_lossy()),
            path = PATH,
            tmp = xml(&self.home.join("tmp").to_string_lossy()),
            storage = xml(&self.storage.to_string_lossy()),
            config = xml(&self.config.to_string_lossy()),
            stdout = xml(&self
                .home
                .join("native-readers.stdout.log")
                .to_string_lossy()),
            stderr = xml(&self
                .home
                .join("native-readers.stderr.log")
                .to_string_lossy()),
        );
        fs::write(&self.plist, body).expect("native-reader LaunchAgent plist");
    }

    fn bootstrap(&self) {
        let output = Command::new("/bin/launchctl")
            .args(["bootstrap", &self.domain])
            .arg(&self.plist)
            .output()
            .expect("launchctl bootstrap runs");
        assert!(
            output.status.success(),
            "launchctl bootstrap failed: {}",
            said(&output)
        );
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", PATH)
            .env("TMPDIR", self.home.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "probierz-native-readers")
            .env("STADO_CONFIG", &self.config);
        command
    }

    fn converge(&self) -> Output {
        self.command(&[
            "release",
            "converge-local-readers",
            "--name",
            "stado",
            "--archive",
            self.archive.to_str().expect("fixture archive path"),
            "--sha256",
            &self.archive_sha256,
        ])
        .output()
        .expect("built Stado convergence command runs")
    }

    fn update_private_reader(&self) -> Output {
        self.command(&[
            "service",
            "update",
            &self.label,
            "--host",
            HOST,
            "--from-archive",
            self.archive.to_str().expect("fixture archive path"),
            "--refresh-image",
            "--json",
        ])
        .output()
        .expect("built Stado service update command runs")
    }
    fn label_print(&self) -> Output {
        self.command(&[
            "service",
            "label-print",
            &self.label,
            "--host",
            HOST,
            "--domain",
            "user",
            "--json",
        ])
        .output()
        .expect("public label-print command runs")
    }

    fn observed_image(&self, expected_pid: u32) -> serde_json::Value {
        let output = self.label_print();
        assert!(
            output.status.success(),
            "label-print failed for pid {expected_pid}: {}",
            said(&output)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!("label-print returned non-JSON ({error}): {}", said(&output))
            });
        assert_eq!(
            report["loaded"], true,
            "label-print did not find the loaded fixture"
        );
        assert_eq!(
            report["domain"], self.domain,
            "label-print did not report the observed launchd owner"
        );
        assert_eq!(
            report["pid"],
            expected_pid.to_string(),
            "label-print and launchd disagree about the live pid"
        );
        assert_eq!(
            report["process_identity_unavailable"],
            serde_json::Value::Null,
            "the public reader could not establish the live process identity: {report}"
        );
        report
    }

    fn pid(&self) -> Option<u32> {
        let service = format!("{}/{}", self.domain, self.label);
        let output = Command::new("/bin/launchctl")
            .args(["print", &service])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.trim().strip_prefix("pid = ")?.trim().parse().ok())
    }

    fn wait_for_pid(&self, different_from: Option<u32>, budget: Duration) -> u32 {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(pid) = self.pid() {
                if different_from != Some(pid) {
                    return pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{} never acquired a replacement pid different from {different_from:?}",
                self.label
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_until_listening(&self, budget: Duration) {
        let address = format!("127.0.0.1:{}", self.port).parse().unwrap();
        let deadline = Instant::now() + budget;
        loop {
            if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the fixture dashboard never listened on {address}: {}",
                fs::read_to_string(self.home.join("native-readers.stderr.log")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn assert_dashboard_serves_product_route(&self) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("connect to fixture dashboard");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("dashboard read timeout");
        stream
            .write_all(b"GET /join.sh HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .expect("write dashboard request");
        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .expect("read dashboard answer");
        assert!(
            answer.starts_with("HTTP/1.1 200 OK"),
            "the real dashboard did not serve /join.sh: {answer}"
        );
    }

    fn declared_program(&self) -> String {
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(&self.plist)
            .output()
            .expect("plutil reads the fixture declaration");
        assert!(output.status.success(), "plutil failed: {}", said(&output));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn cleanup(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        match self
            .command(&[
                "service",
                "bootout",
                &self.label,
                "--host",
                HOST,
                "--domain",
                "user",
                "--json",
            ])
            .output()
        {
            Ok(bootout) => {
                let bootout_report: Option<serde_json::Value> =
                    serde_json::from_slice(&bootout.stdout).ok();
                let bootout_state = bootout_report
                    .as_ref()
                    .and_then(|report| report["state"].as_str());
                if !bootout.status.success()
                    || !matches!(bootout_state, Some("booted_out" | "absent"))
                {
                    failures.push(format!(
                        "Stado service bootout did not prove cleanup: {}",
                        said(&bootout)
                    ));
                }
            }
            Err(error) => failures.push(format!("Stado cleanup bootout did not run: {error}")),
        }

        // A lifecycle refusal is retained as a failure even if the exact-owner
        // launchctl fallback protects the host from a leaked KeepAlive job.
        if !failures.is_empty() || self.pid().is_some() {
            let service = format!("{}/{}", self.domain, self.label);
            match Command::new("/bin/launchctl")
                .args(["bootout", &service])
                .output()
            {
                Ok(fallback) if fallback.status.success() && self.pid().is_none() => {}
                Ok(fallback) => failures.push(format!(
                    "exact-owner fallback did not remove {service}: {}",
                    said(&fallback)
                )),
                Err(error) => failures.push(format!(
                    "fallback launchctl bootout did not run for {service}: {error}"
                )),
            }
        }

        let plist = self.plist.to_string_lossy().into_owned();
        match self
            .command(&["host", "remove-file", HOST, &plist, "--json"])
            .output()
        {
            Ok(remove) => {
                let remove_report: Option<serde_json::Value> =
                    serde_json::from_slice(&remove.stdout).ok();
                let remove_status = remove_report
                    .as_ref()
                    .and_then(|report| report["status"].as_str());
                if !remove.status.success()
                    || !matches!(remove_status, Some("removed" | "absent"))
                    || self.plist.exists()
                {
                    failures.push(format!(
                        "Stado guarded remove-file did not prove cleanup: {}",
                        said(&remove)
                    ));
                }
            }
            Err(error) => failures.push(format!("Stado guarded remove-file did not run: {error}")),
        }

        if failures.is_empty() {
            self.cleanup_finished = true;
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleanup_finished {
            if let Err(error) = self.cleanup() {
                eprintln!("native-reader fixture cleanup failure: {error}");
            }
        }
    }
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("hostname runs");
    assert!(
        output.status.success(),
        "hostname failed: {}",
        said(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn available_login_domain() -> String {
    let uid = unsafe { nix::libc::geteuid() };
    // Prefer user so a live GUI domain cannot hide a wrong-domain reload.
    for domain in [format!("user/{uid}"), format!("gui/{uid}")] {
        let output = Command::new("/bin/launchctl")
            .args(["print", &domain])
            .output()
            .expect("launchctl domain probe runs");
        if output.status.success() {
            return domain;
        }
    }
    panic!("launchd exposes neither the gui/{uid} nor user/{uid} login domain");
}

fn unused_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve a loopback port");
    listener.local_addr().expect("loopback address").port()
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn said(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct FileIdentity {
    device: u64,
    inode: u64,
    sha256: String,
}

fn file_identity(path: &Path) -> FileIdentity {
    let mut file = fs::File::open(path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
    let metadata = file
        .metadata()
        .unwrap_or_else(|error| panic!("cannot identify {}: {error}", path.display()));
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("cannot hash {}: {error}", path.display()));
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        sha256: hex::encode(hash.finalize()),
    }
}

fn assert_maps(
    fixture: &Fixture,
    pid: u32,
    expected_path: &Path,
    expected: &FileIdentity,
) -> serde_json::Value {
    let report = fixture.observed_image(pid);
    assert_eq!(
        report["process_device"], expected.device,
        "public label-print observed the wrong mapped device: {report}"
    );
    assert_eq!(
        report["process_inode"], expected.inode,
        "public label-print observed the wrong mapped inode: {report}"
    );
    assert_eq!(
        report["process_executable"],
        expected_path.to_string_lossy().into_owned(),
        "public label-print observed the wrong executable path: {report}"
    );
    assert_eq!(
        report["process_sha256"], expected.sha256,
        "public label-print did not hash the copied executable bytes: {report}"
    );
    report
}

#[test]
#[ignore = "Probierz runs the real launchd reader lifecycle on a dedicated macOS host"]
fn convergence_reloads_a_cached_private_stado_definition_once() {
    let mut fixture = Fixture::new();
    let private_identity = file_identity(&fixture.private_binary);
    let root_identity = file_identity(&fixture.root_binary);
    assert_ne!(
        (private_identity.device, private_identity.inode),
        (root_identity.device, root_identity.inode),
        "the private and delivered Stado copies must be distinct files"
    );

    fixture.bootstrap();
    fixture.wait_until_listening(Duration::from_secs(60));
    fixture.assert_dashboard_serves_product_route();
    let private_pid = fixture.wait_for_pid(None, Duration::from_secs(30));
    assert_maps(
        &fixture,
        private_pid,
        &fixture.private_binary,
        &private_identity,
    );

    // launchd still holds the private ProgramArguments it bootstrapped, while
    // its source of truth on disk now names the delivered root.
    fixture.write_plist(&fixture.root_binary);
    assert_eq!(
        fixture.declared_program(),
        fixture.root_binary.to_string_lossy().into_owned(),
        "the on-disk plist must name the delivered root"
    );
    assert_eq!(
        fixture.pid(),
        Some(private_pid),
        "rewriting the plist alone must not replace the loaded process"
    );
    let cached = assert_maps(
        &fixture,
        private_pid,
        &fixture.private_binary,
        &private_identity,
    );
    assert_eq!(
        cached["program"],
        fixture.private_binary.to_string_lossy().into_owned(),
        "launchd must still hold the private cached program before convergence"
    );

    let first = fixture.converge();
    assert!(
        first.status.success(),
        "first convergence failed: {}",
        said(&first)
    );
    let root_pid = fixture.wait_for_pid(Some(private_pid), Duration::from_secs(60));
    fixture.wait_until_listening(Duration::from_secs(60));
    let reloaded = assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);
    assert_eq!(
        reloaded["program"],
        fixture.root_binary.to_string_lossy().into_owned(),
        "convergence did not reload the changed ProgramArguments"
    );
    fixture.assert_dashboard_serves_product_route();

    let second = fixture.converge();
    assert!(
        second.status.success(),
        "repeated convergence failed: {}",
        said(&second)
    );
    assert_eq!(
        fixture.wait_for_pid(None, Duration::from_secs(30)),
        root_pid,
        "a process already mapping the delivered root was restarted again"
    );
    let repeated = assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);
    assert_eq!(
        repeated["program"],
        fixture.root_binary.to_string_lossy().into_owned(),
        "repeated convergence changed the loaded definition"
    );
    fixture
        .cleanup()
        .unwrap_or_else(|error| panic!("native-reader cleanup was not proven: {error}"));
}

#[test]
#[ignore = "Probierz runs the real launchd reader lifecycle on a dedicated macOS host"]
fn service_update_reloads_a_cached_global_stado_definition_once() {
    let mut fixture = Fixture::new();
    fixture.write_plist(&fixture.root_binary);
    fixture.bootstrap();
    fixture.wait_until_listening(Duration::from_secs(60));
    let root_pid = fixture.wait_for_pid(None, Duration::from_secs(30));
    let root_identity = file_identity(&fixture.root_binary);
    assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);

    let first = fixture.update_private_reader();
    assert!(
        first.status.success(),
        "private service update failed: {}",
        said(&first)
    );
    let private_path = fixture
        .home
        .join(".stado/services")
        .join(&fixture.label)
        .join("current/darwin-arm/stado");
    assert_eq!(
        fixture.declared_program(),
        private_path.to_string_lossy(),
        "service update did not move the declaration to its installed private tree"
    );
    let private_image = fs::canonicalize(&private_path).expect("installed private Stado exists");
    let private_identity = file_identity(&private_image);
    assert_eq!(private_identity.sha256, root_identity.sha256);
    let private_pid = fixture.wait_for_pid(Some(root_pid), Duration::from_secs(60));
    fixture.wait_until_listening(Duration::from_secs(60));
    assert_maps(&fixture, private_pid, &private_image, &private_identity);
    fixture.assert_dashboard_serves_product_route();

    let second = fixture.update_private_reader();
    assert!(
        second.status.success(),
        "repeated private update failed: {}",
        said(&second)
    );
    assert_eq!(
        fixture.wait_for_pid(None, Duration::from_secs(30)),
        private_pid,
        "replaying the same archive restarted an already-current private reader"
    );
    assert_maps(&fixture, private_pid, &private_image, &private_identity);
    fixture
        .cleanup()
        .unwrap_or_else(|error| panic!("private-reader cleanup was not proven: {error}"));
}

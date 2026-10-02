//! Named credential reads against an isolated real Skarbiec HTTP broker.
//! SKARBIEC_TEST_BIN selects the Skarbiec executable under qualification.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Output, Stdio};
use tempfile::TempDir;

pub(super) struct Run {
    home: TempDir,
    keyring: TempDir,
    output: PathBuf,
    environment: BTreeMap<String, String>,
    skarbiec: String,
    server: Option<Child>,
    server_log: Option<BufReader<ChildStderr>>,
    report: Value,
    pub(super) completed: bool,
}

impl Run {
    pub(super) fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let runs = root.join("target/credential-tests");
        fs::create_dir_all(&runs).unwrap();
        let output = runs.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&output).unwrap();
        let home = tempfile::tempdir_in(&output).unwrap();
        // GPG's socket must fit macOS sun_path. Only its isolated keyring uses
        // the product-owned short-path exception; reports remain in target/.
        let keyrings = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".stado/test-runs");
        fs::create_dir_all(&keyrings).unwrap();
        let keyring = tempfile::tempdir_in(keyrings).unwrap();
        let path = |suffix| home.path().join(suffix).to_string_lossy().into_owned();
        let environment = BTreeMap::from([
            ("PATH".into(), std::env::var("PATH").unwrap()),
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "GNUPGHOME".into(),
                keyring.path().to_string_lossy().into_owned(),
            ),
            ("STADO_CONFIG".into(), path("config.json")),
            ("SKARBIEC_VAULT_FILE".into(), path("vault.json")),
            ("SKARBIEC_AUDIT_FILE".into(), path("audit.jsonl")),
            (
                "SKARBIEC_CAP_SOCKET".into(),
                keyring
                    .path()
                    .join("broker.sock")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("WC_STORAGE_BACKEND".into(), "local".into()),
            ("WC_LOCAL_STORAGE_PATH".into(), path("storage")),
        ]);
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(result.status.success());
            String::from_utf8(result.stdout).unwrap()
        };
        let report = json!({
            "source_revision": git(&["rev-parse", "HEAD"]).trim(),
            "source_diff": git(&["diff", "HEAD"]),
            "stado": env!("CARGO_BIN_EXE_stado"),
            "commands": [], "server_startup": [], "result": "not_run",
        });
        Self {
            home,
            keyring,
            output,
            environment,
            report,
            completed: false,
            skarbiec: std::env::var("SKARBIEC_TEST_BIN").unwrap_or_else(|_| "skarbiec".into()),
            server: None,
            server_log: None,
        }
    }

    fn command(&mut self, binary: &str, args: &[&str], input: Option<&str>) -> Output {
        let mut child = Command::new(binary)
            .args(args)
            .env_clear()
            .envs(&self.environment)
            .current_dir(self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "binary": binary, "args": args, "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        output
    }

    pub(super) fn vault(&mut self, args: &[&str], input: Option<&str>) -> Value {
        let result = self.command(&self.skarbiec.clone(), args, input);
        assert!(
            result.status.success(),
            "Skarbiec {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }

    pub(super) fn store(&mut self, item: &str, token: &str) {
        let document = json!({
            "schema": "skarbiec.item.v2", "kind": "bundle",
            "fields": {"token": token, "private": "not-granted"}, "context": {},
        });
        self.vault(
            &["set-json", item, "--type", "bundle"],
            Some(&document.to_string()),
        );
    }

    pub(super) fn start(&mut self, item: &str, decoy: &str) {
        let token_file = self.home.path().join("consumer-token");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&token_file)
            .unwrap()
            .write_all(uuid::Uuid::new_v4().to_string().as_bytes())
            .unwrap();
        self.vault(
            &[
                "grant",
                "issue",
                "stado",
                "--capabilities",
                &format!("read:{item}#token,read:{decoy}#token"),
                "--token-file",
                token_file.to_str().unwrap(),
            ],
            None,
        );
        let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reserved.local_addr().unwrap().port().to_string();
        let config = json!({
            "credentials": {"store": "skarbiec"},
            "secrets": {"skarbiec": {
                "url": format!("http://127.0.0.1:{port}"),
                "consumer": "stado", "token_file": token_file,
            }},
        });
        fs::write(&self.environment["STADO_CONFIG"], config.to_string()).unwrap();
        drop(reserved);
        let mut server = Command::new(&self.skarbiec)
            .args(["serve", "--port", &port])
            .env_clear()
            .envs(&self.environment)
            .current_dir(self.home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        self.server_log = Some(BufReader::new(server.stderr.take().unwrap()));
        self.server = Some(server);
        loop {
            let mut line = String::new();
            let read = self
                .server_log
                .as_mut()
                .unwrap()
                .read_line(&mut line)
                .unwrap();
            assert_ne!(read, 0, "Skarbiec exited before binding: {}", self.report);
            self.report["server_startup"]
                .as_array_mut()
                .unwrap()
                .push(json!(line));
            if line.contains(&format!(
                "skarbiec API listening on http://127.0.0.1:{port}"
            )) {
                break;
            }
        }
    }

    pub(super) fn get(&mut self, item: &str, field: &str) -> Output {
        self.command(
            env!("CARGO_BIN_EXE_stado"),
            &["credentials", "get", item, "--field", field],
            None,
        )
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let mut cleanup_ok = true;
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            let exit = server.wait();
            cleanup_ok &= exit.is_ok();
            self.report["server_cleanup"] = json!({
                "exit_status": exit.as_ref().ok().and_then(|status| status.code()),
                "error": exit.err().map(|error| error.to_string()),
            });
        }
        if let Some(log) = &mut self.server_log {
            let mut remaining = String::new();
            let read = log.read_to_string(&mut remaining);
            cleanup_ok &= read.is_ok();
            self.report["server_shutdown"] = json!(remaining);
            self.report["server_log_error"] = json!(read.err().map(|error| error.to_string()));
        }
        let cleanup = Command::new("gpgconf")
            .args(["--kill", "all"])
            .env_clear()
            .envs(&self.environment)
            .output();
        cleanup_ok &= cleanup.as_ref().is_ok_and(|output| output.status.success());
        self.report["gpg_cleanup"] = match cleanup {
            Ok(output) => {
                json!({"exit_status": output.status.code(), "stderr": String::from_utf8_lossy(&output.stderr)})
            }
            Err(error) => json!({"error": error.to_string()}),
        };
        for (name, path) in [
            ("home_cleanup", self.home.path()),
            ("keyring_cleanup", self.keyring.path()),
        ] {
            let result = fs::remove_dir_all(path);
            cleanup_ok &= result.is_ok();
            self.report[name] =
                json!({"path": path, "error": result.err().map(|error| error.to_string())});
        }
        self.report["result"] = json!(
            if self.completed && cleanup_ok && !std::thread::panicking() {
                "passed"
            } else {
                "failed"
            }
        );
        let report = self.output.join("report.json");
        let written = serde_json::to_vec_pretty(&self.report)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&report, bytes));
        match &written {
            Ok(()) => eprintln!("credential read report: {}", report.display()),
            Err(error) => eprintln!(
                "cannot write credential read report {}: {error}",
                report.display()
            ),
        }
        if !std::thread::panicking() {
            assert!(
                cleanup_ok && written.is_ok(),
                "credential fixture cleanup or evidence persistence failed: {}",
                report.display()
            );
        }
    }
}

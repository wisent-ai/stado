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

struct Run {
    home: TempDir,
    _keyring: TempDir,
    output: PathBuf,
    environment: BTreeMap<String, String>,
    skarbiec: String,
    server: Option<Child>,
    server_log: Option<BufReader<ChildStderr>>,
    report: Value,
    completed: bool,
}

impl Run {
    fn new() -> Self {
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
            _keyring: keyring,
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

    fn vault(&mut self, args: &[&str], input: Option<&str>) -> Value {
        let result = self.command(&self.skarbiec.clone(), args, input);
        assert!(
            result.status.success(),
            "Skarbiec {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }

    fn store(&mut self, item: &str, token: &str) {
        let document = json!({
            "schema": "skarbiec.item.v2", "kind": "bundle",
            "fields": {"token": token, "private": "not-granted"}, "context": {},
        });
        self.vault(
            &["set-json", item, "--type", "bundle"],
            Some(&document.to_string()),
        );
    }

    fn start(&mut self, item: &str, decoy: &str) {
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

    fn get(&mut self, item: &str, field: &str) -> Output {
        self.command(
            env!("CARGO_BIN_EXE_stado"),
            &["credentials", "get", item, "--field", field],
            None,
        )
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            if let Ok(status) = server.wait() {
                self.report["server_exit_status"] = json!(status.code());
            }
        }
        if let Some(log) = &mut self.server_log {
            let mut remaining = String::new();
            let _ = log.read_to_string(&mut remaining);
            self.report["server_shutdown"] = json!(remaining);
        }
        let cleanup = Command::new("gpgconf")
            .args(["--kill", "all"])
            .env_clear()
            .envs(&self.environment)
            .output();
        self.report["gpg_cleanup"] = match cleanup {
            Ok(output) => {
                json!({"exit_status": output.status.code(), "stderr": String::from_utf8_lossy(&output.stderr)})
            }
            Err(error) => json!({"error": error.to_string()}),
        };
        self.report["result"] = json!(if self.completed && !std::thread::panicking() {
            "passed"
        } else {
            "failed"
        });
        let report = self.output.join("report.json");
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.report) {
            let _ = fs::write(&report, bytes);
        }
        eprintln!("credential read report: {}", report.display());
    }
}

#[test]
fn named_field_read_does_not_select_a_same_named_role_or_broaden_its_grant() {
    let mut run = Run::new();
    let owner = "Credential Test <credential@example.invalid>";
    run.vault(&["init", owner], None);
    let item = uuid::Uuid::new_v4().simple().to_string();
    let decoy = uuid::Uuid::new_v4().simple().to_string();
    let expected = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    run.store(&item, &expected);
    run.store(&decoy, &other);
    run.start(&item, &decoy);
    let saved = run.vault(&["get", &item], None);
    assert_eq!(saved["fields"]["token"], expected);
    let untagged = run.get(&item, "token");
    assert!(
        untagged.status.success(),
        "{}",
        String::from_utf8_lossy(&untagged.stderr)
    );
    assert_eq!(untagged.stdout, format!("{expected}\n").as_bytes());

    run.vault(
        &["retag", &decoy, "--tags", &format!("stado:role:{item}")],
        None,
    );
    let shadowed = run.get(&item, "token");
    assert!(
        shadowed.status.success(),
        "{}",
        String::from_utf8_lossy(&shadowed.stderr)
    );
    assert_eq!(shadowed.stdout, format!("{expected}\n").as_bytes());

    let denied = run.get(&item, "private");
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    let diagnostic = String::from_utf8(denied.stderr).unwrap();
    assert!(
        diagnostic.contains(&item) && diagnostic.contains("private"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("not-granted"));
    assert_eq!(run.vault(&["get", &item], None), saved);
    run.completed = true;
}

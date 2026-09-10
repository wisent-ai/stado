//! A native API mutation must reach the real store, and confirmation must
//! precede that mutation. The server and independent reader are real Stado
//! processes; the only state they may reach belongs to this run.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const OBJECT: &str = "stado://operator-input/artifacts/request.txt";
const CONTENT: &str = "Native input reaches the object store.\nZażółć gęślą jaźń.\n";
const STARTUP_SECONDS: u64 = 30;
const POLL_MILLIS: u64 = 50;

struct NativeApi {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    child: Child,
    address: String,
}

impl NativeApi {
    async fn start() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.wisent-output/operator-input")
            .join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        let storage = root.join("storage");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&storage).unwrap();
        fs::create_dir_all(root.join("temporary")).unwrap();
        let config = root.join("config.json");
        fs::write(&config, json!({"storage": {"backend": "local", "local": {"path": storage}}}).to_string()).unwrap();
        fs::write(storage.join("registry.json"), json!({"schema_version": 2, "targets": [], "coordinators": []}).to_string()).unwrap();
        let binary = env!("CARGO_BIN_EXE_stado");
        let version = Command::new(binary).arg("--version").output().unwrap();
        assert!(version.status.success());
        fs::write(root.join("source.json"), json!({
            "binary": binary,
            "version": String::from_utf8_lossy(&version.stdout),
            "binary_sha256": hex::encode(Sha256::digest(fs::read(binary).unwrap()))
        }).to_string()).unwrap();
        let log = root.join("server.stderr");
        let child = Self::command(&root, &home, &config)
            .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
            .stdout(fs::File::create(root.join("server.stdout")).unwrap())
            .stderr(fs::File::create(&log).unwrap())
            .spawn().unwrap();
        let mut api = Self { root, home, config, child, address: String::new() };
        let deadline = Instant::now() + Duration::from_secs(STARTUP_SECONDS);
        loop {
            assert!(api.child.try_wait().unwrap().is_none(), "real Stado server exited; evidence {}", api.root.display());
            if let Some(address) = fs::read_to_string(&log).unwrap().lines()
                .find_map(|line| line.strip_prefix("[dashboard] listening on ")) {
                api.address = address.trim().to_string();
                break;
            }
            assert!(Instant::now() < deadline, "real Stado server did not listen; evidence {}", api.root.display());
            tokio::time::sleep(Duration::from_millis(POLL_MILLIS)).await;
        }
        println!("native input evidence: {}", api.root.display());
        api
    }

    fn command(root: &Path, home: &Path, config: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command.env_clear().env("HOME", home).env("TMPDIR", root.join("temporary"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", config).env("STADO_API_URL", "")
            .env("WC_PROVIDERS", "local").env("NO_COLOR", "1")
            .stdin(Stdio::null());
        command
    }

    fn cli(&self, name: &str, args: &[&str]) -> std::process::Output {
        let output = Self::command(&self.root, &self.home, &self.config).args(args).output().unwrap();
        fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        fs::write(self.root.join(format!("{name}.process.json")), json!({"args": args, "exit_code": output.status.code()}).to_string()).unwrap();
        output
    }

    async fn request(&self, name: &str, body: Value) -> (u16, Value) {
        let response = reqwest::Client::new().post(format!("{}/api/operator/run", self.address))
            .header("X-Stado-Action", "operator-command").json(&body).send().await.unwrap();
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap();
        fs::write(self.root.join(format!("{name}.json")), json!({"request": body, "http_status": status, "response": payload}).to_string()).unwrap();
        (status, payload)
    }
}

impl Drop for NativeApi {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn native_input_writes_only_after_confirmation_and_is_read_back_independently() {
    let api = NativeApi::start().await;
    let args = ["storage", "put", OBJECT, "-", "--json"];
    let (status, refusal) = api.request("unconfirmed", json!({"args": args, "stdin": CONTENT})).await;
    assert_eq!(status, 403, "{refusal}");
    let before = api.cli("before", &["storage", "stat", OBJECT, "--json"]);
    assert!(before.status.success(), "{}", String::from_utf8_lossy(&before.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&before.stdout).unwrap()["state"], "absent");

    let (status, receipt) = api.request("write", json!({
        "args": args, "stdin": CONTENT, "confirmation": "RUN_MUTATION"
    })).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["ok"], true, "{receipt}");
    let destination = api.root.join("downloaded.txt");
    let downloaded = api.cli("read-back", &["storage", "get", OBJECT, destination.to_str().unwrap()]);
    assert!(downloaded.status.success(), "{}", String::from_utf8_lossy(&downloaded.stderr));
    assert_eq!(fs::read(&destination).unwrap(), CONTENT.as_bytes());

    let (status, removed) = api.request("remove", json!({
        "args": ["storage", "rm", OBJECT, "--json"], "confirmation": "RUN_MUTATION"
    })).await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["ok"], true, "{removed}");
    let after = api.cli("after", &["storage", "stat", OBJECT, "--json"]);
    assert!(after.status.success(), "{}", String::from_utf8_lossy(&after.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&after.stdout).unwrap()["state"], "absent");
}

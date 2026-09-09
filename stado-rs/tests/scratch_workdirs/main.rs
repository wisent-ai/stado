//! Drive cleanup through the real CLI, retaining receipts outside the swept home.

use serde_json::Value;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    evidence: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let evidence_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/scratch-workdirs");
        fs::create_dir_all(&evidence_root).unwrap();
        let evidence = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(evidence_root)
            .unwrap()
            .keep();
        let home = evidence.join("home");
        fs::create_dir_all(home.join(".stado/work")).unwrap();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(evidence.join("revision.txt"), revision.stdout).unwrap();
        Self { evidence, home }
    }

    fn work(&self) -> PathBuf {
        self.home.join(".stado/work")
    }

    fn run(&self, step: &str, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", self.home.join("absent-config.json"))
            .output()
            .unwrap();
        fs::write(self.evidence.join(format!("{step}.stdout")), &output.stdout).unwrap();
        fs::write(self.evidence.join(format!("{step}.stderr")), &output.stderr).unwrap();
        fs::write(
            self.evidence.join(format!("{step}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"arguments": args, "exitCode": output.status.code()}),
            )
            .unwrap(),
        )
        .unwrap();
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid report: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn apply_removes_all_directories_including_previously_exempt_names() {
    let fixture = Fixture::new();
    let work = fixture.work();
    let names = ["alpha", "jobs", "runs", "run-signals"];
    for name in names {
        fs::create_dir_all(work.join(name).join("nested")).unwrap();
        fs::write(work.join(name).join("nested/contents"), name).unwrap();
    }
    fs::write(work.join("loose.txt"), "preserve loose file").unwrap();
    let outside = fixture.home.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "outside data").unwrap();
    symlink(&outside, work.join("escape")).unwrap();
    symlink(&outside, work.join("alpha/nested/escape")).unwrap();

    let preview = fixture.run("preview", &["workdirs", "--json"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    for name in names {
        assert_eq!(
            fs::read(work.join(name).join("nested/contents")).unwrap(),
            name.as_bytes()
        );
    }
    let removed = fixture.run("apply", &["workdirs", "--apply", "--json"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    for name in names {
        assert!(
            !work.join(name).exists(),
            "directory {name} was left behind"
        );
    }
    assert_eq!(
        fs::read_to_string(outside.join("keep")).unwrap(),
        "outside data"
    );
    assert_eq!(
        fs::read_to_string(work.join("loose.txt")).unwrap(),
        "preserve loose file"
    );
    assert!(fs::symlink_metadata(work.join("escape"))
        .unwrap()
        .file_type()
        .is_symlink());
    let repeated = fixture.run("repeat", &["workdirs", "--apply", "--json"]);
    assert!(repeated.status.success());
    assert_eq!(document(&repeated)["removed"], serde_json::json!([]));
}

#[test]
fn linked_root_is_refused_without_touching_its_target() {
    let fixture = Fixture::new();
    let outside = fixture.home.join("outside");
    fs::create_dir_all(outside.join("owned")).unwrap();
    fs::write(outside.join("owned/data"), "do not delete").unwrap();
    fs::remove_dir(fixture.work()).unwrap();
    symlink(&outside, fixture.work()).unwrap();
    let output = fixture.run("linked-root", &["workdirs", "--apply", "--json"]);
    assert!(!output.status.success(), "linked root was accepted");
    assert_eq!(
        fs::read_to_string(outside.join("owned/data")).unwrap(),
        "do not delete"
    );
    assert!(!document(&output)["failed"].as_array().unwrap().is_empty());
}

#[test]
fn unreadable_tree_is_a_failure_in_both_output_formats() {
    let fixture = Fixture::new();
    let locked = fixture.work().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("data"), "retain on refusal").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o0)).unwrap();
    let text = fixture.run("unreadable-text", &["workdirs", "--apply"]);
    let json = fixture.run("unreadable-json", &["workdirs", "--apply", "--json"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!text.status.success(), "text command hid the failure");
    assert!(!json.status.success(), "JSON command hid the failure");
    assert!(!document(&json)["failed"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(locked.join("data")).unwrap(),
        "retain on refusal"
    );
}

#[test]
fn missing_root_is_empty_but_regular_file_root_is_not() {
    let fixture = Fixture::new();
    fs::remove_dir(fixture.work()).unwrap();
    let absent = fixture.run("absent", &["workdirs", "--json"]);
    assert!(absent.status.success());
    assert_eq!(document(&absent)["rootPresent"], false);
    fs::write(fixture.work(), "not a directory").unwrap();
    let invalid = fixture.run("invalid-root", &["workdirs", "--apply", "--json"]);
    assert!(
        !invalid.status.success(),
        "non-directory root was called absent"
    );
    assert_eq!(
        fs::read_to_string(fixture.work()).unwrap(),
        "not a directory"
    );
}

struct Dashboard(std::process::Child);
impl Drop for Dashboard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn desktop_api_requires_confirmation_and_returns_the_actual_removal() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.work().join("jobs/one")).unwrap();
    fs::write(fixture.work().join("jobs/one/result"), "real job data").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let _server = Dashboard(
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args([
                "dashboard",
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .env_clear()
            .env("HOME", &fixture.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", fixture.home.join("absent-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", fixture.home.join("storage"))
            .stdout(fs::File::create(fixture.evidence.join("server.stdout")).unwrap())
            .stderr(fs::File::create(fixture.evidence.join("server.stderr")).unwrap())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::Client::new();
    let origin = format!("http://127.0.0.1:{port}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while client
        .get(format!("{origin}/healthz"))
        .send()
        .await
        .is_err()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "dashboard did not start: {}",
            fixture.evidence.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let budget = 1200u64;
    for (step, args, confirmation, status) in [
        ("api-preview", vec!["workdirs", "--json"], "", 200u16),
        (
            "api-refusal",
            vec!["workdirs", "--apply", "--json"],
            "",
            403u16,
        ),
        (
            "api-apply",
            vec!["workdirs", "--apply", "--json"],
            "RUN_MUTATION",
            200u16,
        ),
    ] {
        let response = client.post(format!("{origin}/api/operator/run"))
            .header("X-Stado-Action", "operator-command")
            .json(&serde_json::json!({"args": args, "confirmation": confirmation, "timeout_seconds": budget}))
            .send().await.unwrap();
        let actual_status = response.status().as_u16();
        let body = response.text().await.unwrap();
        fs::write(fixture.evidence.join(format!("{step}.json")), &body).unwrap();
        fs::write(
            fixture.evidence.join(format!("{step}.status")),
            actual_status.to_string(),
        )
        .unwrap();
        assert_eq!(actual_status, status, "{body}");
        if step != "api-apply" {
            assert_eq!(
                fs::read_to_string(fixture.work().join("jobs/one/result")).unwrap(),
                "real job data"
            );
        } else {
            let receipt: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(receipt["exit_code"], 0, "{body}");
            assert!(!fixture.work().join("jobs").exists(), "{body}");
        }
    }
}

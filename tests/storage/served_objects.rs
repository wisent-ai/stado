//! A `stado://` object written by a process on the host that serves the
//! store lands where the object API reads it. One isolated deployment: a
//! local store whose served queue namespace (`ecosystem/probierz/`) exists,
//! as it does on every host an object API serves, and the real `stado`
//! executable as a queue client of that store with no object API configured.
//! A release object put through it must sit at
//! `<store>/ecosystem/releases/<key>` — the address `GET /api/release/object`
//! reads — and nowhere under the queue namespace, and the same executable
//! must see it, read it back and refuse to replace it.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const URI: &str = "stado://releases/served-objects-test/0.0.1/darwin-arm64/release.json";
const KEY: &str = "served-objects-test/0.0.1/darwin-arm64/release.json";

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/storage-served-objects")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        Self {
            root,
            report: json!({"commands": [], "outcome": "failed"}),
        }
    }

    fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn cli(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn a_release_written_on_the_serving_host_lands_where_the_object_api_reads_it() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let source = deployment.root.join("release.json");
    fs::write(&source, b"{\"published\":true}\n").unwrap();
    let source = source.to_string_lossy().into_owned();

    deployment.cli(&["storage", "put", URI, &source]);

    let served = deployment.store().join("ecosystem/releases").join(KEY);
    let misplaced = deployment
        .store()
        .join("ecosystem/probierz/ecosystem/releases")
        .join(KEY);
    assert_eq!(
        fs::read(&served).ok().as_deref(),
        Some(&b"{\"published\":true}\n"[..]),
        "the object is at the address the object API serves: {}",
        served.display()
    );
    assert!(
        !misplaced.exists(),
        "nothing is written under the queue namespace: {}",
        misplaced.display()
    );

    let stat = deployment.cli(&["storage", "stat", URI, "--json"]);
    let stat: Value = serde_json::from_str(&stat).unwrap();
    assert_eq!(stat["state"], "present", "{stat}");

    let read = deployment.root.join("read.json");
    let read_path = read.to_string_lossy().into_owned();
    deployment.cli(&["storage", "get", URI, &read_path]);
    assert_eq!(fs::read(&read).unwrap(), b"{\"published\":true}\n");

    let replacement = deployment.root.join("replacement.json");
    fs::write(&replacement, b"{\"published\":false}\n").unwrap();
    let replacement = replacement.to_string_lossy().into_owned();
    let refused = deployment.run(&["storage", "put", URI, &replacement]);
    assert!(
        !refused.status.success(),
        "a release object is never replaced"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("release objects are immutable"),
        "the refusal names why: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!(
        "storage-served-objects evidence: {}",
        deployment.root.display()
    );
}

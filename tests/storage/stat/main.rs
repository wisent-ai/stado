//! `stado storage stat` answers one object's own facts. One isolated
//! deployment: a local store and the real `stado` executable as its client
//! with no object API configured. Two objects whose names share a prefix are
//! put with different content types and bodies; stat of the shorter name
//! answers its own size and content type, never the longer one's, and no
//! metadata error; stat of a name that only prefixes both is absent, and its
//! exit status says the question was answered. Every command, its exit
//! status and its output are kept in the case's report.json under the
//! package's build directory.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SHORT: &str = "stado://journey/stat/blob";
const LONG: &str = "stado://journey/stat/blob-longer";
const PREFIX: &str = "stado://journey/stat/bl";

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
            .join(".build/storage-stat")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            report: json!({"commands": [], "outcome": "failed"}),
        }
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
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

    fn put(&mut self, uri: &str, body: &[u8], content_type: &str) {
        let source = self.root.join("source");
        fs::write(&source, body).unwrap();
        let source = source.to_string_lossy().into_owned();
        self.cli(&[
            "storage",
            "put",
            uri,
            &source,
            "--content-type",
            content_type,
        ]);
    }

    fn stat(&mut self, uri: &str) -> Value {
        serde_json::from_str(&self.cli(&["storage", "stat", uri, "--json"])).unwrap()
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
fn stat_answers_the_named_object_and_not_one_its_name_prefixes() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let short = b"short body\n";
    let long = b"a longer body than the short one\n";
    deployment.put(SHORT, short, "text/plain");
    deployment.put(LONG, long, "application/json");

    let stat = deployment.stat(SHORT);
    assert_eq!(stat["state"], "present", "{stat}");
    assert_eq!(stat["size"], json!(short.len()), "its own size: {stat}");
    assert_eq!(stat["metadata"]["content-type"], "text/plain", "{stat}");
    assert_eq!(stat["metadata_error"], Value::Null, "{stat}");
    assert!(stat["updated_at"].is_string(), "its update time: {stat}");

    let longer = deployment.stat(LONG);
    assert_eq!(longer["size"], json!(long.len()), "{longer}");
    assert_eq!(
        longer["metadata"]["content-type"], "application/json",
        "{longer}"
    );

    let prefix = deployment.stat(PREFIX);
    assert_eq!(
        prefix["state"], "absent",
        "a name that only prefixes objects: {prefix}"
    );
    assert_eq!(prefix["metadata"], json!({}), "{prefix}");

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("storage-stat evidence: {}", deployment.root.display());
}

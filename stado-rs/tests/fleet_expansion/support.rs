//! Reusable real-binary journey; the only economic numbers are declared inputs.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub struct Journey {
    pub root: PathBuf,
    pub store: PathBuf,
    binary: PathBuf,
}
impl Journey {
    pub fn new() -> Self {
        let base = std::env::var_os("STADO_EXPANSION_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fleet-expansion-runs")
            });
        fs::create_dir_all(&base).unwrap();
        let root = tempfile::Builder::new()
            .prefix("expansion-")
            .tempdir_in(base)
            .unwrap()
            .keep();
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        if let Ok(revision) = std::env::var("WISENT_SOURCE_COMMIT") {
            assert_eq!(revision.len(), 40, "release source revision must be exact");
            assert!(revision.bytes().all(|b| b.is_ascii_hexdigit()));
            fs::write(root.join("revision.txt"), revision).unwrap();
            fs::write(
                root.join("source.sha256"),
                std::env::var("WISENT_SOURCE_SHA256").unwrap(),
            )
            .unwrap();
        } else {
            for (name, args) in [
                ("revision.txt", vec!["rev-parse", "HEAD"]),
                ("source.patch", vec!["diff", "--binary", "HEAD"]),
            ] {
                let out = Command::new("git")
                    .args(args)
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .output()
                    .unwrap();
                assert!(out.status.success());
                fs::write(root.join(name), out.stdout).unwrap();
            }
        }
        let binary = std::env::var_os("STADO_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_stado")));
        let j = Self {
            root,
            store,
            binary,
        };
        let out = j.invoke(
            &["registry", "push", "-"],
            Some(&json!({"schema_version":2,"targets":[],"coordinators":[]}).to_string()),
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        println!("retained evidence: {}", j.root.display());
        j
    }
    pub fn invoke(&self, args: &[&str], input: Option<&str>) -> Output {
        let mut child = Command::new(&self.binary)
            .args(args)
            .env_clear()
            .env("HOME", &self.root)
            .env("TMPDIR", &self.root)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.root.join("absent.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.store)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
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
        let out = child.wait_with_output().unwrap();
        let record = json!({"binary":self.binary,"args":args,"stdin":input,"exit":out.status.code(),"stdout":String::from_utf8_lossy(&out.stdout),"stderr":String::from_utf8_lossy(&out.stderr)});
        fs::write(
            self.root
                .join(format!("command-{}.json", uuid::Uuid::new_v4())),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        out
    }
    pub fn demand(&self) {
        let path = self.root.join("gui-plan.json");
        fs::write(
            &path,
            json!({"schema":"wisent.gui-automation-plan.v1","operation":"enable"}).to_string(),
        )
        .unwrap();
        assert!(!self
            .invoke(
                &[
                    "workload",
                    "run",
                    "gui-automation",
                    "--plan",
                    path.to_str().unwrap()
                ],
                None
            )
            .status
            .success());
        let out = self.invoke(&["fleet", "needs", "--json"], None);
        assert!(out.status.success());
        assert!(document(&out)["needs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["platform"] == "darwin-arm64"));
    }
    pub fn set(&self, options: Vec<Value>, version: Option<&str>) -> Output {
        let mut args = vec!["fleet", "expansion", "set", "--document", "-", "--json"];
        if let Some(v) = version {
            args.extend(["--expect-version", v]);
        }
        self.invoke(
            &args,
            Some(&json!({"schema_version":1,"options":options}).to_string()),
        )
    }
    pub fn plan(&self, budget: &str, months: &str) -> Output {
        self.invoke(
            &[
                "fleet",
                "expansion",
                "plan",
                "--budget-usd",
                budget,
                "--horizon-months",
                months,
                "--json",
            ],
            None,
        )
    }
    pub fn persisted(&self, report: &Value) -> Value {
        let path = self
            .store
            .join("state/fleet/expansion/plans")
            .join(format!("{}.json", report["plan_id"].as_str().unwrap()));
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }
}
pub fn document(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {} / {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
pub fn option(id: &str, upfront: f64, monthly: f64, saving: f64) -> Value {
    let now = chrono::Utc::now();
    json!({"id":id,"label":id,"kind":"buy","need_keys":["host:darwin-arm64"],"benefit_group":id,
        "upfront_usd":upfront,"monthly_cost_usd":monthly,"monthly_savings_usd":saving,"monthly_margin_usd":0,"lead_time_days":0,
        "evidence":"Explicit scenario for a real refused GUI placement; no vendor quote or measured income",
        "observed_at":(now-chrono::Duration::minutes(1)).to_rfc3339(),"valid_until":(now+chrono::Duration::days(1)).to_rfc3339()})
}

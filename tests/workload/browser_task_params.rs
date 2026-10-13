//! A `weles-browser-task` plan may carry `params`, the action's own
//! parameters, and the real `stado` binary of this checkout refuses, before
//! any host is contacted, a `params` that is not an object and a `params`
//! that names a parameter the task sets itself. The plan reader runs in an
//! isolated home; the report keeps every command, its exit status and its
//! streams beside the source revision.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Run {
    root: PathBuf,
    home: PathBuf,
    report: Value,
}

impl Run {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/workload-browser-task-params")
            .join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let source_revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repository)
            .output()
            .unwrap();
        let report = json!({
            "source_revision": String::from_utf8_lossy(&source_revision.stdout).trim(),
            "commands": [],
            "outcome": "failed",
        });
        Self { root, home, report }
    }

    fn plan(&self, name: &str, plan: &Value) -> PathBuf {
        let path = self.root.join(format!("{name}.plan.json"));
        fs::write(&path, serde_json::to_vec_pretty(plan).unwrap()).unwrap();
        path
    }

    fn run_plan(&mut self, plan: &PathBuf) -> std::process::Output {
        let plan = plan.to_str().unwrap();
        let args = [
            "workload",
            "run",
            "weles-browser-task",
            "--target",
            "no-such-host",
            "--plan",
            plan,
        ];
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado/config.json"))
            .args(args)
            .stdin(Stdio::null())
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

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    fn passed(mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
        eprintln!("report: {}", self.root.join("report.json").display());
    }
}

fn base_plan() -> Value {
    json!({
        "schema": "wisent.weles-browser-task-plan.v1",
        "action": "overleaf_version_history_scan",
        "url": "https://www.overleaf.com/project",
        "objective": "Read the version history for one phrase.",
        "session_label": "params-probe",
    })
}

#[test]
fn params_that_are_not_an_object_or_that_name_a_task_field_are_refused_before_any_host() {
    let mut run = Run::start();

    let mut not_object = base_plan();
    not_object["params"] = json!("project=abc");
    let plan = run.plan("not-object", &not_object);
    let output = run.run_plan(&plan);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a string params parsed: {stderr}");
    assert!(
        stderr.contains("workload plan params must be an object of the action's own parameters"),
        "the refusal names the shape:\n{stderr}"
    );

    let mut reserved = base_plan();
    reserved["params"] = json!({"project": "abc", "url": "https://elsewhere.invalid/"});
    let plan = run.plan("reserved", &reserved);
    let output = run.run_plan(&plan);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a reserved name parsed: {stderr}");
    assert!(
        stderr.contains("workload plan params names \"url\", which the task itself sets"),
        "the refusal names the taken parameter:\n{stderr}"
    );

    let mut accepted = base_plan();
    accepted["params"] = json!({"project": "abc", "query_text": "a phrase"});
    let plan = run.plan("accepted", &accepted);
    let output = run.run_plan(&plan);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("workload plan params"),
        "a sound params object passed the plan reader and reached host resolution:\n{stderr}"
    );
    assert!(
        stderr.contains("no-such-host"),
        "the run went on to the named host and was refused there:\n{stderr}"
    );
    run.passed();
}

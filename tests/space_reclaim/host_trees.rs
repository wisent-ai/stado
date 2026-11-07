//! The host tree register gate (`host-trees-register`): the Stado verbs that
//! write on a host, run for real in an isolated home with a declared work
//! root, leave no tree that no register entry holds. The listing is the
//! product's own inventory, `stado space report --unregistered TARGET
//! --revision REV --json`, of this machine, the one host of the isolated
//! registry; a writer that makes an undeclared tree, by whatever spelling,
//! fails here naming the verb that ran before the tree appeared and its
//! path. A tree made by hand outside the register shows up, so the gate can
//! fail, and the inventory refuses a binary not built from the revision it is
//! asked about. The report keeps the source revision and every command with
//! its exit status and streams.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Run {
    root: PathBuf,
    home: PathBuf,
    work_root: PathBuf,
    report: Value,
}

impl Run {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/host-trees-register")
            .join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        let work_root = root.join("work-root");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&work_root).unwrap();
        let source_revision = match std::env::var("WISENT_SOURCE_COMMIT") {
            Ok(commit) => commit,
            Err(_) => {
                let output = Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&repository)
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
        };
        let report = json!({
            "source_revision": source_revision,
            "commands": [],
            "outcome": "failed",
        });
        Self {
            root,
            home,
            work_root,
            report,
        }
    }

    fn stado(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado/config.json"))
            .env("STADO_WORK_ROOT", &self.work_root)
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

    fn success(&mut self, args: &[&str]) -> String {
        let output = self.stado(args);
        assert!(
            output.status.success(),
            "stado {args:?} exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// The revision `stado --version` says this binary was built from.
    fn revision(&mut self) -> String {
        let version = self.success(&["--version"]);
        version
            .split("(rev ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .unwrap_or_else(|| panic!("stado --version names no revision: {version}"))
            .to_string()
    }

    /// The trees below the isolated home and work root no register entry
    /// holds, as the product's inventory lists them for `target`.
    fn unregistered(&mut self, target: &str, revision: &str) -> Vec<String> {
        let output = self.stado(&[
            "space",
            "report",
            "--unregistered",
            target,
            "--revision",
            revision,
            "--json",
        ]);
        let answer: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "the inventory printed no JSON ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(answer["unread"], json!([]), "{answer}");
        let work_root = self.work_root.to_string_lossy().into_owned();
        answer["instances"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|instance| instance["path"].as_str())
            .filter(|path| path.starts_with('~') || path.starts_with(&work_root))
            .map(str::to_string)
            .collect()
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

#[test]
fn every_host_writer_verb_leaves_only_registered_trees() {
    let mut run = Run::start();
    run.success(&["config", "init"]);
    let registry: Value = serde_json::from_str(&run.success(&["registry", "pull"])).unwrap();
    let target = registry["targets"][0]["name"]
        .as_str()
        .expect("the first-run registry names this machine")
        .to_string();
    let work_root = run.work_root.to_string_lossy().into_owned();
    run.success(&[
        "registry",
        "set",
        "--path",
        "targets.0.work_root",
        "--value",
        &work_root,
        "--json",
    ]);
    let revision = run.revision();

    let writers: Vec<Vec<&str>> = vec![
        vec![
            "space",
            "reclaim",
            "--local",
            "--apply",
            "--reason",
            "host tree register gate",
        ],
        vec!["space", "candidates", "--json"],
        vec!["space", "report", &target, "--json"],
    ];
    for writer in &writers {
        run.stado(writer);
        let unregistered = run.unregistered(&target, &revision);
        assert!(
            unregistered.is_empty(),
            "after stado {writer:?} these trees are held by no register entry: {unregistered:?}"
        );
    }

    // A tree no writer declares is found wherever it is made.
    let rogue = run.home.join(".stado/undeclared-tree");
    fs::create_dir_all(&rogue).unwrap();
    let found = run.unregistered(&target, &revision);
    assert_eq!(found, vec!["~/.stado/undeclared-tree".to_string()]);

    // The answer is about the register this binary carries.
    let other = run.stado(&[
        "space",
        "report",
        "--unregistered",
        &target,
        "--revision",
        "deadbeef",
        "--json",
    ]);
    assert!(!other.status.success(), "another revision was answered");
    let refusal = String::from_utf8_lossy(&other.stderr).into_owned()
        + &String::from_utf8_lossy(&other.stdout);
    assert!(
        refusal.contains("this stado was built from") && refusal.contains("not deadbeef"),
        "the refusal does not name both revisions: {refusal}"
    );
    run.passed();
}

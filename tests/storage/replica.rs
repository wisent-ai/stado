//! A disaster-recovery backup on the primary store's own volume is refused by
//! both writers. One isolated deployment: `config init` seeds a local store,
//! the backup is declared as a directory beside it on the same disk, and the
//! real `stado` executable is asked to replicate (`storage backup`) and to
//! open the store (`storage ls`). Neither may write a single object to the
//! backup, and each says why.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const REFUSAL: &str = "is on the same volume as the primary";

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
            .join(".build/storage-replica")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        Self {
            root,
            report: json!({"commands": [], "outcome": "failed"}),
        }
    }

    fn backup(&self) -> PathBuf {
        self.root.join(".stado").join("local-backup")
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
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

    fn cli(&mut self, args: &[&str]) {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

fn files_under(path: &Path) -> usize {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path)
            } else {
                1
            }
        })
        .sum()
}

#[test]
fn a_backup_on_the_primary_volume_is_never_written() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let backup = deployment.backup().to_string_lossy().into_owned();
    deployment.cli(&["config", "set", "storage.backup.local.path", &backup]);
    deployment.cli(&["config", "set", "storage.backup.backend", "local"]);

    let replication = deployment.run(&["storage", "backup"]);
    let stderr = String::from_utf8_lossy(&replication.stderr).into_owned();
    assert!(
        !replication.status.success(),
        "replicating onto the primary's own volume must be refused: {stderr}"
    );
    assert!(
        stderr.contains(REFUSAL),
        "the refusal names the cause: {stderr}"
    );

    let listing = deployment.run(&["storage", "ls"]);
    let stderr = String::from_utf8_lossy(&listing.stderr).into_owned();
    assert!(
        stderr.contains(REFUSAL),
        "the inline mirror reports why it writes nothing: {stderr}"
    );

    assert_eq!(
        files_under(&deployment.backup()),
        0,
        "no object may reach a backup on the primary's own volume"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("storage-replica evidence: {}", deployment.root.display());
}

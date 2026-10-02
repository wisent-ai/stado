use std::cell::RefCell;
use std::fs::{self, File, FileTimes};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::{json, Value};

pub struct Native {
    root: PathBuf,
    run: PathBuf,
    pub home: PathBuf,
    pub cache_root: PathBuf,
    pub registry: PathBuf,
    pub policy: Value,
    commands: RefCell<Vec<Value>>,
    observations: RefCell<Vec<Value>>,
}

impl Native {
    pub fn new(case: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let run = root
            .join("build/real-tests/disk-cleanup")
            .join(format!("{case}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(run.parent().unwrap()).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&run).unwrap();
        let home = run.join("home");
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        let cache_root = home.join("cleanup-caches");
        fs::create_dir(&cache_root).unwrap();
        let capacity = fs2::total_space(&home)
            .unwrap()
            .div_ceil(1024 * 1024 * 1024);
        let policy = json!({
            "mode": "enforce", "check_interval_seconds": 60,
            "low_free_gb": capacity + 1, "target_free_gb": capacity + 2,
            "max_items_per_pass": 10, "max_bytes_per_pass": 1048576, "max_scan_items": 1000,
            "cleaners": {"build_caches": {"root": cache_root, "min_age_seconds": 86400}}
        });
        let mut this = Self {
            registry: home.join(".stado/local-storage/registry.json"),
            root,
            run,
            home,
            cache_root,
            policy,
            commands: RefCell::new(Vec::new()),
            observations: RefCell::new(Vec::new()),
        };
        let revision = this.command("git", &["rev-parse", "HEAD"], false);
        assert!(revision.status.success());
        let revision = String::from_utf8(revision.stdout)
            .unwrap()
            .trim()
            .to_owned();
        let version = this.success(&["--version"]);
        this.observe(
            "source",
            json!({"checkout_revision": revision, "binary_version": version}),
        );
        assert!(
            version.contains(&revision),
            "native binary does not identify checkout {revision}: {version}"
        );
        this.success(&["config", "init"]);
        let configuration = this.json(&["config", "show", "--json"]);
        assert_eq!(
            configuration["file"],
            this.home.join(".stado/config.json").to_str().unwrap()
        );
        let effective = &configuration["resolved"];
        assert_eq!(
            effective["wc_storage_backend"], "local",
            "primary storage is not isolated local storage"
        );
        assert_eq!(
            effective["wc_backup_storage_backend"], "local",
            "backup storage is not isolated local storage"
        );
        this.registry = this
            .storage_path(effective["wc_local_storage_path"].as_str().unwrap())
            .join("registry.json");
        this.storage_path(effective["wc_backup_local_storage_path"].as_str().unwrap());
        this.observe("configuration", configuration);
        let mut document = this.json(&["registry", "pull"]);
        assert_eq!(
            document["targets"].as_array().unwrap().len(),
            1,
            "first-run registry must be isolated"
        );
        let mut target = document["targets"][0].take();
        let original_name = target["name"].take();
        target["name"] = json!("example-cleanup-host");
        let aliases = target["hostnames"].as_array_mut().unwrap();
        if !aliases.contains(&original_name) {
            aliases.push(original_name);
        }
        target["disk_cleanup"] = this.policy.clone();
        let body = serde_json::to_string(&target).unwrap();
        this.success(&[
            "registry",
            "set",
            "--path",
            "targets.0",
            "--value",
            &body,
            "--json",
        ]);
        let persisted: Value = serde_json::from_slice(&fs::read(&this.registry).unwrap()).unwrap();
        assert_eq!(
            persisted["targets"][0], target,
            "declaration did not reach local storage"
        );
        this
    }

    fn storage_path(&self, raw: &str) -> PathBuf {
        let path = match raw.strip_prefix("~/") {
            Some(relative) => self.home.join(relative),
            None => PathBuf::from(raw),
        };
        assert!(
            path.is_absolute() && path.starts_with(&self.home),
            "storage escapes isolated home: {raw}"
        );
        assert!(
            !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
            "storage contains parent traversal: {raw}"
        );
        path
    }

    fn command(&self, program: &str, args: &[&str], isolated: bool) -> Output {
        let mut command = Command::new(program);
        command.args(args).current_dir(&self.root);
        if isolated {
            command
                .env_clear()
                .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
                .env("HOME", &self.home)
                .env("STADO_CONFIG", self.home.join(".stado/config.json"));
        }
        let output = command.output().expect("execute real native command");
        self.commands.borrow_mut().push(json!({
            "program": program, "args": args, "isolated": isolated,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        output
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_stado"), args, true)
    }

    pub fn success(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?} exited {:?}: {}\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.success(args)).expect("native command emitted JSON")
    }

    pub fn observe(&self, operation: &str, value: Value) {
        self.observations
            .borrow_mut()
            .push(json!({"operation": operation, "value": value}));
    }

    pub fn cleanup(&self) -> Value {
        let body = self.success(&["disk-cleanup", "--to-target"]);
        let documents: Vec<Value> = serde_json::Deserializer::from_str(&body)
            .into_iter::<Value>()
            .collect::<Result<_, _>>()
            .expect("native cleanup receipts");
        let mut cleanup = documents
            .into_iter()
            .filter(|value| value.get("cleaners").is_some());
        let report = cleanup.next().expect("one disk cleanup receipt");
        assert!(cleanup.next().is_none(), "ambiguous disk cleanup receipts");
        self.observe("cleanup", report.clone());
        report
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".cache/wisent-compute")
    }

    pub fn cache(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# Regenerable native regression-test cache.\n").unwrap();
        fs::write(path.join("payload"), b"regenerable test data\n").unwrap();
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
    }

    pub fn set_policy(&self, policy: &Value) {
        let body = serde_json::to_string(policy).unwrap();
        self.success(&[
            "registry",
            "set",
            "--path",
            "targets.example-cleanup-host.disk_cleanup",
            "--value",
            &body,
            "--json",
        ]);
        let persisted: Value = serde_json::from_slice(&fs::read(&self.registry).unwrap()).unwrap();
        assert_eq!(&persisted["targets"][0]["disk_cleanup"], policy);
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        let panicking = std::thread::panicking();
        let state = self.state_dir().join("disk-cleanup-state.json");
        let persisted = match fs::read_to_string(&state) {
            Ok(body) => json!({"path": state, "body": body}),
            Err(error) => json!({"path": state, "read_error": error.to_string()}),
        };
        let cleanup = fs::remove_dir_all(&self.home);
        let report = json!({
            "scope": "Real native cleanup and filesystem state; no graphical qualification or builds by the test",
            "verdict": if panicking || cleanup.is_err() { "failed" } else { "passed" },
            "cwd": self.root, "home": self.home,
            "commands": self.commands.borrow().as_slice(),
            "observations": self.observations.borrow().as_slice(),
            "persisted_state": persisted,
            "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
        });
        let destination = self.run.join("report.json");
        let written = fs::write(&destination, serde_json::to_vec_pretty(&report).unwrap())
            .and_then(|()| fs::set_permissions(&destination, fs::Permissions::from_mode(0o600)));
        eprintln!("cleanup evidence: {}", destination.display());
        if !panicking {
            written.expect("retain native cleanup evidence");
            cleanup.expect("remove only the isolated test home");
        } else if let Err(error) = written {
            eprintln!("cannot retain cleanup evidence: {error}");
        }
    }
}

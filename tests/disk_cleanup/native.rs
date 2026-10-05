//! One isolated host for the disk-full rule: a real APFS volume of its own,
//! attached from a sparse disk image inside the checkout's build directory
//! and used as the test's `$HOME`, so the janitor's `statvfs` reads a volume
//! the test can fill past 80% without touching the machine's own disk.

use std::cell::RefCell;
use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::{json, Value};

/// Size of the test volume. Small enough to fill in a moment, large enough
/// for the local registry and storage `config init` writes.
const VOLUME_MEGABYTES: u64 = 256;

pub struct Native {
    root: PathBuf,
    run: PathBuf,
    pub home: PathBuf,
    pub registry: PathBuf,
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
        fs::create_dir_all(&run).unwrap();
        let home = run.join("home");
        fs::create_dir(&home).unwrap();
        let mut this = Self {
            registry: PathBuf::new(),
            root,
            run,
            home,
            commands: RefCell::new(Vec::new()),
            observations: RefCell::new(Vec::new()),
        };
        let image = this.run.join("volume.sparseimage");
        let size = format!("{VOLUME_MEGABYTES}m");
        let volume_name = format!("stado-rule-{case}");
        this.tool(
            "/usr/bin/hdiutil",
            &[
                "create",
                "-quiet",
                "-size",
                &size,
                "-fs",
                "APFS",
                "-type",
                "SPARSE",
                "-volname",
                &volume_name,
                image.to_str().unwrap(),
            ],
        );
        this.tool(
            "/usr/bin/hdiutil",
            &[
                "attach",
                "-quiet",
                "-nobrowse",
                "-owners",
                "on",
                "-mountpoint",
                this.home.to_str().unwrap(),
                image.to_str().unwrap(),
            ],
        );
        fs::set_permissions(&this.home, fs::Permissions::from_mode(0o700)).unwrap();
        let revision = this.tool("git", &["rev-parse", "HEAD"]);
        let version = this.success(&["--version"]);
        this.observe(
            "source",
            json!({"checkout_revision": revision.trim(), "binary_version": version}),
        );
        this.success(&["config", "init"]);
        let configuration = this.json(&["config", "show", "--json"]);
        let effective = &configuration["resolved"];
        assert_eq!(
            effective["wc_storage_backend"], "local",
            "primary storage is not isolated local storage"
        );
        this.registry = this
            .storage_path(effective["wc_local_storage_path"].as_str().unwrap())
            .join("registry.json");
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
        path
    }

    fn record(&self, program: &str, args: &[&str], output: &Output) {
        self.commands.borrow_mut().push(json!({
            "program": program, "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
    }

    /// A host tool the harness needs, run outside the isolated home.
    fn tool(&self, program: &str, args: &[&str]) -> String {
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
        self.record(program, args, &output);
        assert!(
            output.status.success(),
            "{program} {args:?} exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let program = env!("CARGO_BIN_EXE_stado");
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado/config.json"))
            .output()
            .expect("execute real native command");
        self.record(program, args, &output);
        output
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

    /// One `stado disk-cleanup` pass, and its report.
    pub fn cleanup(&self) -> Value {
        let report = self.json(&["disk-cleanup"]);
        self.observe("cleanup", report.clone());
        report
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".cache/wisent-compute")
    }

    /// A directory its build tool tagged as regenerable.
    pub fn cache(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# Regenerable native regression-test cache.\n").unwrap();
        fs::write(path.join("payload"), b"regenerable test data\n").unwrap();
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
    }

    /// Write the user's own data until the volume is past the disk-full
    /// threshold, and return where it is.
    pub fn fill_with_user_data(&self) -> PathBuf {
        let documents = self.home.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        let path = documents.join("user-data.bin");
        let total = fs2::total_space(&self.home).unwrap();
        let free = fs2::available_space(&self.home).unwrap();
        let keep_free = total / 100 * 15;
        let mut file = File::create(&path).unwrap();
        let block = vec![0x5a_u8; 1024 * 1024];
        let mut written = 0_u64;
        while written + keep_free < free {
            file.write_all(&block).unwrap();
            written += block.len() as u64;
        }
        file.sync_all().unwrap();
        let used = 100.0 * (1.0 - fs2::available_space(&self.home).unwrap() as f64 / total as f64);
        self.observe(
            "user data",
            json!({"path": path, "bytes": written, "used_percent": used}),
        );
        assert!(used >= 80.0, "the test volume is only {used:.1}% used");
        path
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
        let detach = Command::new("/usr/bin/hdiutil")
            .args(["detach", "-quiet", "-force", self.home.to_str().unwrap()])
            .output();
        let detached = matches!(&detach, Ok(output) if output.status.success());
        let removed = detached
            .then(|| fs::remove_file(self.run.join("volume.sparseimage")))
            .transpose();
        let report = json!({
            "scope": "Real native cleanup on an attached APFS volume; no graphical qualification or builds by the test",
            "verdict": if panicking || !detached { "failed" } else { "passed" },
            "cwd": self.root, "home": self.home,
            "commands": self.commands.borrow().as_slice(),
            "observations": self.observations.borrow().as_slice(),
            "persisted_state": persisted,
            "detached": detached,
            "image_removed": matches!(removed, Ok(Some(()))),
        });
        let destination = self.run.join("report.json");
        let written = fs::write(&destination, serde_json::to_vec_pretty(&report).unwrap())
            .and_then(|()| fs::set_permissions(&destination, fs::Permissions::from_mode(0o600)));
        eprintln!("cleanup evidence: {}", destination.display());
        if !panicking {
            written.expect("retain native cleanup evidence");
            assert!(detached, "detach the test volume");
        } else if let Err(error) = written {
            eprintln!("cannot retain cleanup evidence: {error}");
        }
    }
}

//! Real APFS volume isolated under this checkout's build directory.

use std::cell::RefCell;
use std::fs::{self, File, FileTimes};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::{json, Value};
use stado::primitives::file_mode::widen;
#[path = "native/fill.rs"]
mod fill;

/// Each case's volume, as the space documentation's real tests declare it:
/// https://stado.wisent.com/docs/capabilities/space#real-tests
const VOLUME_MEGABYTES: u64 = 256;
/// Every permission for the owner alone, the isolated home's mode.
const OWNER_ALL: u32 = widen(nix::libc::S_IRWXU);
/// Read and write for the owner alone, the retained report's mode.
const OWNER_READ_WRITE: u32 = widen(nix::libc::S_IRUSR | nix::libc::S_IWUSR);

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
            .join("build/real-tests/space-reclaim")
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
        fs::set_permissions(&this.home, fs::Permissions::from_mode(OWNER_ALL)).unwrap();
        // The quality gate runs this journey in an exported tree with no Git
        // checkout and states the commit it exported (`WISENT_SOURCE_COMMIT`);
        // run from the checkout, the commit is the checkout's own.
        let revision = match std::env::var("WISENT_SOURCE_COMMIT") {
            Ok(commit) => commit,
            Err(std::env::VarError::NotPresent) => this.tool("git", &["rev-parse", "HEAD"]),
            Err(error) => panic!("WISENT_SOURCE_COMMIT is set but cannot be read: {error}"),
        };
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
        // The isolated scan ceiling keeps unrelated builds from skipping reclaim.
        let ceiling = f64::MAX.to_string();
        this.success(&["config", "set", "host.scan_load_ceiling", &ceiling]);
        this.observe(
            "scan_load_ceiling",
            json!({"declared": ceiling, "key": "host.scan_load_ceiling"}),
        );
        let mut document = this.json(&["registry", "pull"]);
        let [target] = document["targets"].as_array_mut().unwrap().as_mut_slice() else {
            panic!("first-run registry must be isolated");
        };
        let mut target = target.take();
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

    pub fn record(&self, program: &str, args: &[&str], output: &Output) {
        self.commands.borrow_mut().push(json!({
            "program": program, "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
    }

    /// A host tool the harness needs, run outside the isolated home.
    fn tool(&self, program: &str, args: &[&str]) -> String {
        let output = match Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output()
        {
            Ok(output) => output,
            Err(error) => panic!("cannot run {program}: {error}"),
        };
        self.record(program, args, &output);
        assert!(
            output.status.success(),
            "{program} {args:?} exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// The real `stado` with `args` in the isolated home, not yet started.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado/config.json"));
        // Run as a gate of an admitted build, the journey is part of that
        // build and inherits its admission, so what it proves does not hang
        // on which Oko this machine has installed.
        let admitted =
            stado::cli::release_loop::allowed::inherited(stado::cli::release_loop::allowed::BUILD);
        if let Some(value) = std::env::var_os(&admitted) {
            command.env(admitted, value);
        }
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let output = self
            .command(args)
            .output()
            .expect("execute real native command");
        self.record(env!("CARGO_BIN_EXE_stado"), args, &output);
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

    pub fn cleanup(&self) -> Value {
        let report = self.json(&[
            "space",
            "reclaim",
            "--local",
            "--apply",
            "--reason",
            "isolated cleanup test",
        ]);
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
            .and_then(|()| {
                fs::set_permissions(&destination, fs::Permissions::from_mode(OWNER_READ_WRITE))
            });
        eprintln!("cleanup evidence: {}", destination.display());
        if !panicking {
            written.expect("retain native cleanup evidence");
            assert!(detached, "detach the test volume");
        } else if let Err(error) = written {
            eprintln!("cannot retain cleanup evidence: {error}");
        }
    }
}

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub struct Run {
    pub root: PathBuf,
    pub source: PathBuf,
    pub output: PathBuf,
    pub path: std::ffi::OsString,
    report: Value,
}

pub fn digest(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    format!("{:x}", hash.finalize())
}

impl Run {
    pub fn new() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = repository
            .join(".build")
            .join(format!("schema-{}", &id[..12]));
        fs::create_dir_all(root.join("home")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::create_dir_all(root.join("tmp")).unwrap();
        let source = root.join("source");
        fs::create_dir_all(source.join("migrations")).unwrap();
        let output = root.join("output");
        fs::create_dir_all(&output).unwrap();
        let mut run = Self {
            root,
            source,
            output,
            path: std::env::var_os("PATH").unwrap(),
            report: json!({"outcome": "failed", "commands": []}),
        };
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(revision.status.success());
        run.report["source_revision"] = json!(String::from_utf8(revision.stdout).unwrap().trim());
        let patch = Command::new("git")
            .current_dir(&repository)
            .args(["diff", "--binary", "HEAD"])
            .output()
            .unwrap();
        assert!(patch.status.success());
        fs::write(run.root.join("source.patch"), &patch.stdout).unwrap();
        run.report["source_patch_sha256"] = json!(format!("{:x}", Sha256::digest(&patch.stdout)));
        run.report["binary_sha256"] = json!(digest(Path::new(env!("CARGO_BIN_EXE_stado"))));
        run.save();
        let mut command = run.stado();
        command.arg("--version");
        let version = run.success(command);
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .any(|part| Some(part) == run.report["source_revision"].as_str()),
            "candidate source revision differs"
        );
        run.report["binary_version"] = json!(version);
        run.save();
        run
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("LC_ALL", "C")
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env("WISENT_OUTPUT_DIR", &self.output)
            .env("WISENT_SOURCE_DIR", &self.source)
            .env("WISENT_PRODUCT", "schema-release-journey")
            .env("WISENT_VERSION", "1.0.0")
            .current_dir(&self.source)
            .stdin(Stdio::null());
        command
    }

    pub fn stado(&self) -> Command {
        self.command(env!("CARGO_BIN_EXE_stado"))
    }

    pub fn run(&mut self, mut command: Command) -> Output {
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let result = command.output();
        let record = match &result {
            Ok(output) => json!({"program": command.get_program().to_string_lossy(), "args": args,
                "exit_status": output.status.code(), "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr)}),
            Err(error) => {
                json!({"program": command.get_program().to_string_lossy(), "args": args, "error": error.to_string()})
            }
        };
        self.report["commands"].as_array_mut().unwrap().push(record);
        self.save();
        result.expect("real dependency could not run; see report.json")
    }

    pub fn success(&mut self, command: Command) -> String {
        let result = self.run(command);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }

    pub fn refuse(&mut self, command: Command, cause: &str) {
        let result = self.run(command);
        assert!(!result.status.success(), "operation unexpectedly succeeded");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(cause),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    pub fn bundle(&mut self) {
        let mut command = self.stado();
        command.args([
            "product",
            "source-bundle",
            "--name",
            "database-schema.tar",
            "--include",
            "migrations",
        ]);
        self.success(command);
    }

    pub fn release(&self) -> (PathBuf, String) {
        let path = self
            .root
            .join(format!("release-{}.tar", uuid::Uuid::new_v4()));
        let mut archive = tar::Builder::new(File::create(&path).unwrap());
        archive
            .append_path_with_name(
                self.output.join("release/database-schema.tar"),
                "database-schema.tar",
            )
            .unwrap();
        archive.finish().unwrap();
        drop(archive);
        let sha256 = digest(&path);
        (path, sha256)
    }

    pub fn pass(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

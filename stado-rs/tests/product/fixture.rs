//! Real Stado and qualified SDK executables; isolated consumer data, retained proof.

use serde_json::{json, Value};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub struct Journey {
    root: Option<tempfile::TempDir>,
    pub home: PathBuf,
    pub evidence: PathBuf,
    binary: PathBuf,
    config: Option<PathBuf>,
    sequence: Cell<u32>,
    finished: bool,
}

impl Journey {
    pub fn new() -> Self {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"));
        let scratch = source.join("target/product-runs");
        fs::create_dir_all(&scratch).unwrap();
        let root = tempfile::Builder::new().prefix("product-").tempdir_in(scratch).unwrap();
        let home = root.path().join("home");
        fs::create_dir(&home).unwrap();
        let evidence = std::env::var_os("WISENT_TEST_EVIDENCE_DIR")
            .or_else(|| std::env::var_os("WISENT_OUTPUT_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|| source.join("target/product-evidence"))
            .join("native-product")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&evidence).unwrap();
        let binary = root.path().join("stado");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &binary).unwrap();
        let mut journey = Self {
            root: Some(root), home, evidence, binary, config: None,
            sequence: Cell::new(0), finished: false,
        };
        let compiled_source = option_env!("STADO_SOURCE_REVISION").or(option_env!("WISENT_SOURCE_COMMIT"));
        let (bytes, digest) = stado::release_control::sha256_file(&journey.binary).unwrap();
        journey.retain("source.json", &serde_json::to_vec_pretty(&json!({
            "compiled_source_revision": compiled_source,
            "binary": journey.binary, "binary_bytes": bytes, "binary_sha256": digest,
            "consumer_home": journey.home, "started_at": chrono::Utc::now().to_rfc3339(),
        })).unwrap());
        assert!(compiled_source.is_some_and(|source| source.len() == 40 && source.bytes().all(|byte| byte.is_ascii_hexdigit())),
            "qualification requires the exact source identity recorded by the Stado build worker");
        let mut config = Command::new(&journey.binary);
        config.args(["config", "show"]);
        let output = journey.run(config);
        assert!(output.status.success(), "cannot read the actual selected Stado configuration: {}", stderr(&output));
        let selected: Value = serde_json::from_slice(&output.stdout).unwrap();
        journey.config = Some(PathBuf::from(selected["file"].as_str().expect("stado config show did not name its selected file")));
        let catalog = journey.catalog();
        journey.retain("initial-catalog.json", &serde_json::to_vec_pretty(&catalog).unwrap());
        journey
    }

    pub fn retain(&self, name: &str, bytes: &[u8]) {
        fs::write(self.evidence.join(name), bytes).unwrap();
    }

    fn run(&self, mut command: Command) -> Output {
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        let directory = self.evidence.join(format!("command-{sequence:04}"));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("request.json"), serde_json::to_vec_pretty(&json!({
            "program": command.get_program().to_string_lossy(),
            "argv": command.get_args().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
            "environment_keys": command.get_envs().map(|(key, _)| key.to_string_lossy()).collect::<Vec<_>>(),
        })).unwrap()).unwrap();
        let output = match command.stdin(Stdio::null()).output() {
            Ok(output) => output,
            Err(error) => {
                fs::write(directory.join("spawn-error.txt"), error.to_string()).unwrap();
                panic!("real command could not start; evidence {}: {error}", directory.display());
            }
        };
        fs::write(directory.join("stdout.log"), &output.stdout).unwrap();
        fs::write(directory.join("stderr.log"), &output.stderr).unwrap();
        fs::write(directory.join("exit.json"), serde_json::to_vec(&json!({
            "code": output.status.code(), "status": output.status.to_string(),
        })).unwrap()).unwrap();
        output
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        let mut command = Command::new(&self.binary);
        command.args(args)
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.config.as_ref().expect("selected configuration is required"))
            .env("WISENT_WORKSPACE", self.home.join("workspace"))
            .env("NO_COLOR", "1");
        self.run(command)
    }

    pub fn catalog(&self) -> Value {
        let output = self.stado(&["product", "catalog", "--json"]);
        assert!(output.status.success(), "qualified native product catalog is unavailable: {}", stderr(&output));
        serde_json::from_slice(&output.stdout).expect("the real product catalog is not JSON")
    }

    pub fn sdk_path(&self) -> PathBuf {
        let platform = if cfg!(target_os = "macos") { "darwin-arm64" } else { "linux-amd64" };
        self.home.join(".stado/cache/product-sdk")
            .join(stado::deploy::native_signing::runtime::VERSION)
            .join(platform).join("wisent-products")
    }

    pub fn assert_no_installation(&self) {
        let records = self.home.join(".stado/products");
        match fs::read_dir(&records) {
            Ok(products) => {
                for product in products {
                    let product = product.unwrap();
                    if !product.file_type().unwrap().is_dir() { continue; }
                    for surface in <stado::cli::setup::product::Surface as clap::ValueEnum>::value_variants() {
                        let record = product.path().join(format!("{}.json", surface.as_str()));
                        assert!(!record.try_exists().unwrap(), "an installation was recorded: {}", record.display());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot inspect actual product records {}: {error}", records.display()),
        }
        for directory in [".stado/bin", ".local/bin", "Applications"] {
            assert!(!self.home.join(directory).try_exists().unwrap(), "a product installation destination was created: {directory}");
        }
    }

    pub fn finish(mut self) {
        self.root.take().unwrap().close().expect("consumer cleanup failed");
        self.retain("outcome.json", &serde_json::to_vec_pretty(&json!({
            "status": "passed", "commands": self.sequence.get(),
            "finished_at": chrono::Utc::now().to_rfc3339(), "consumer_removed": true,
        })).unwrap());
        self.finished = true;
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        if !self.finished {
            let outcome = json!({"status": "failed", "commands": self.sequence.get(), "finished_at": chrono::Utc::now().to_rfc3339()});
            if let Err(error) = fs::write(self.evidence.join("outcome.json"), outcome.to_string()) {
                eprintln!("cannot retain failed native product journey {}: {error}", self.evidence.display());
            }
        }
    }
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

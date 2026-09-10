//! The isolated journey: a registry naming this machine, a home and config
//! nothing else shares, the built CLI invoked against them, and the dashboard
//! boundary the same story crosses.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

use crate::story::{native_story, NativeStory, SYSTEM_PATH, TARGET};

pub(crate) fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| panic!("create retained evidence {}: {error}", path.display()));
    file.write_all(bytes)
        .unwrap_or_else(|error| panic!("write retained evidence {}: {error}", path.display()));
}

pub(crate) fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(
        output.status.success(),
        "blocked: the real hostname executable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let hostname = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_string();
    assert!(
        !hostname.is_empty(),
        "the real current host has no hostname"
    );
    hostname
}

pub(crate) struct Journey {
    pub(crate) root: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) registry: PathBuf,
    pub(crate) hostname: String,
    pub(crate) story: NativeStory,
    pub(crate) config_before: Vec<u8>,
    pub(crate) registry_before: Vec<u8>,
}

impl Journey {
    pub(crate) fn new() -> Self {
        let story = native_story();
        let native = fs::metadata(story.program).unwrap_or_else(|error| {
            panic!(
                "blocked: required native logging executable {} is unavailable: {error}",
                story.program
            )
        });
        assert!(
            native.is_file() && native.permissions().mode() & 0o111 != 0,
            "blocked: required native logging dependency {} is not an executable file",
            story.program,
        );

        let evidence =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/host-exec-retained-logs");
        fs::create_dir_all(&evidence).expect("create repository retained-evidence root");
        let root = tempfile::Builder::new()
            .prefix("retained-log-")
            .tempdir_in(&evidence)
            .expect("create repository-rooted retained-log journey")
            .keep();
        let home = root.join("home");
        let storage = root.join("storage");
        let tmp = root.join("tmp");
        for directory in [&home, &storage, &tmp] {
            fs::create_dir_all(directory).expect("create isolated journey directory");
        }
        let config = root.join("config.json");
        write_private(
            &config,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "storage": {
                    "backend": "local",
                    "local": {"path": storage},
                },
            }))
            .unwrap(),
        );

        let hostname = hostname();
        let registry = storage.join("registry.json");
        write_private(
            &registry,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": 2,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": story.platform,
                    "hostnames": [hostname],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        );

        let config_before = fs::read(&config).expect("read isolated config baseline");
        let registry_before = fs::read(&registry).expect("read isolated registry baseline");
        let journey = Self {
            root,
            home,
            storage,
            config,
            registry,
            hostname,
            story,
            config_before,
            registry_before,
        };
        assert!(journey.home.starts_with(&journey.root));
        assert!(journey.config.starts_with(&journey.root));
        assert!(journey.registry.starts_with(&journey.root));
        eprintln!("retained-log evidence: {}", journey.root.display());
        journey
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "host-exec-retained-logs")
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1");
        command
    }

    pub(crate) fn invoke(&self, step: &str, words: &[&str]) -> Output {
        let mut args = vec!["host", "exec", TARGET, "--json", "--"];
        args.extend_from_slice(words);
        let output = self
            .command()
            .args(&args)
            .output()
            .unwrap_or_else(|error| panic!("the built Stado binary did not start: {error}"));
        write_private(&self.root.join(format!("{step}.stdout")), &output.stdout);
        write_private(&self.root.join(format!("{step}.stderr")), &output.stderr);
        write_private(
            &self.root.join(format!("{step}.json")),
            &serde_json::to_vec_pretty(&json!({
                "schema": "stado.host-exec-retained-log-process.v1",
                "binary": env!("CARGO_BIN_EXE_stado"),
                "args": args,
                "exit_code": output.status.code(),
                "success": output.status.success(),
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
                "host": self.hostname,
                "platform": self.story.platform,
                "native_program": self.story.program,
            }))
            .unwrap(),
        );
        output
    }

    pub(crate) fn retain_http(&self, step: &str, status: reqwest::StatusCode, body: &str) {
        write_private(
            &self.root.join(format!("{step}.body.json")),
            body.as_bytes(),
        );
        write_private(
            &self.root.join(format!("{step}.http.json")),
            &serde_json::to_vec_pretty(&json!({
                "schema": "stado.host-exec-retained-log-http.v1",
                "http_status": status.as_u16(),
                "test_source_revision": env!("STADO_SOURCE_REVISION"),
            }))
            .unwrap(),
        );
    }


    pub(crate) fn assert_unchanged(&self) {
        assert_eq!(
            fs::read(&self.config).expect("read isolated config after journey"),
            self.config_before,
            "the read-only journey changed its isolated Stado config",
        );
        assert_eq!(
            fs::read(&self.registry).expect("read isolated registry after journey"),
            self.registry_before,
            "the read-only journey changed its isolated registry",
        );
    }
}

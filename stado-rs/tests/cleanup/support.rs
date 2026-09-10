//! The isolated journey every disk-cleanup story runs: a real cache tree
//! with the tag a janitor looks for, the state file it keeps, and the built
//! binary invoked against both.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub(crate) const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

pub(crate) struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    cache_root: PathBuf,
}

impl Journey {
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/cleanup-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("cleanup-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        let cache_root = home.path().join("build-output");
        fs::create_dir_all(&storage).unwrap();
        fs::create_dir_all(&cache_root).unwrap();

        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        let release_platform = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "darwin-arm64",
            ("linux", "x86_64") => "linux-amd64",
            platform => panic!("unsupported cleanup journey platform: {platform:?}"),
        };
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "cleanup-runner",
                "kind": "local",
                "release_platform": release_platform,
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "enforce",
                    "check_interval_seconds": 60,
                    "low_free_gb": 1000000,
                    "target_free_gb": 1000001,
                    "max_bytes_per_pass": 1073741824_u64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 1000,
                    "max_pass_seconds": 30,
                    "cleaners": {
                        "build_caches": {
                            "min_age_seconds": 86400,
                            "root": cache_root
                        }
                    }
                }
            }],
            "coordinators": []
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        Self {
            home,
            storage,
            cache_root,
        }
    }

    pub(crate) fn command(&self) -> Command {
        let executable = std::env::var_os("STADO_TEST_BINARY")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_stado").into());
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local");
        command
    }

    pub(crate) fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    pub(crate) fn invoke_ok(&self, args: &[&str]) -> Value {
        let output = self.invoke(args);
        assert!(
            output.status.success(),
            "stado {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        serde_json::from_slice(&output.stdout).expect("cleanup prints one JSON report")
    }

    pub(crate) fn tagged_cache(&self, name: &str) -> PathBuf {
        let directory = self.cache_root.join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
        fs::write(directory.join("payload.bin"), vec![0x5a; 8192]).unwrap();
        let touched = Command::new("/usr/bin/touch")
            .args(["-t", "202001010000"])
            .arg(&directory)
            .status()
            .unwrap();
        assert!(
            touched.success(),
            "fixture directory mtime was not backdated"
        );
        directory
    }

    pub(crate) fn untagged_directory(&self) -> PathBuf {
        let directory = self.cache_root.join("source-tree");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("valuable-source.txt"), b"keep\n").unwrap();
        directory
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.home
            .path()
            .join(".cache/wisent-compute/disk-cleanup-state.json")
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.home.path().join(".cache/wisent-compute")
    }

    pub(crate) fn retired_locks(&self) -> Vec<PathBuf> {
        fs::read_dir(self.state_dir())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("disk-cleanup.lock.retired.")
            })
            .collect()
    }
}

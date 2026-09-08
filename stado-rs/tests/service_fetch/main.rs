//! `stado service file-fetch` against real files under a real home.
//!
//! Every case drives the built `stado` binary. The registry target names THIS
//! machine in `hostnames` and declares no remote destination at all, so
//! `deploy/host_channel.rs::target_is_this_host` is true and the product takes
//! its current-host path: the program it would otherwise pipe to a login shell
//! runs here, against files this test made. There is no stand-in executable on
//! PATH and no stubbed digest — the bytes make a real round trip through
//! base64 and a real `/bin/bash`, the host-side SHA-256 is `shasum`'s, and the
//! local one this test computes itself. `HOME` is a tempdir, so the file being
//! copied, the symlink being refused and the oversized file being declined are
//! all real state this test made, and the operator's own `~/.ssh` can never be
//! reached.
//!
//! What is defended: a fetched file is byte-exact where `env-show` of the same
//! file is not — that difference is the whole reason this command exists; the
//! `--json` report carries both digests and never the content; and the
//! refusals — a source that is not there, a host outside the registry, a
//! symlink, a path outside the target home, a file past the transfer limit and
//! a relative destination — leave nothing written.

mod fetch;
mod refusals;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The service label every test addresses. Declared on the target itself, the
/// way `deploy/service.rs::declared_services` reads a registry-managed unit.
const SERVICE: &str = "com.wisent.always-on.weles";

/// The registry document version this fixture declares, so the product's own
/// reader parses it as the current shape.
const REGISTRY_SCHEMA: u32 = 2;

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    /// A registry holding exactly one target: this machine, under the name
    /// `here`, with the managed unit declared on it.
    ///
    /// The host name is lower-cased because the registry refuses a declared
    /// host name that is not normalized, and no destination is declared
    /// because a target that IS this machine needs none.
    fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        let registry = serde_json::json!({
            "schema_version": REGISTRY_SCHEMA,
            "targets": [{
                "name": "here",
                "kind": "local",
                "release_platform": platform(),
                "hostnames": [this_host()],
                "services": [{
                    "label": SERVICE,
                    "name": SERVICE,
                    "kind": "launchd",
                    "path": format!("/Library/LaunchDaemons/{SERVICE}.plist"),
                    "program": "/bin/sh"
                }]
            }],
            "coordinators": []
        });
        std::fs::write(
            fleet.storage.path().join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        fleet
    }

    /// Write a file under the target home and return its absolute path.
    fn file(&self, relative: &str, body: &[u8], mode: u32) -> PathBuf {
        let path = self.home.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        path
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            .output()
            .expect("stado binary runs")
    }

    fn file_fetch(&self, source: &str, extra: &[&str]) -> Output {
        self.fetch_from("here", source, extra)
    }

    /// A fetch against any host name — the refusals need a name the registry
    /// does not hold.
    fn fetch_from(&self, host: &str, source: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "service",
            "file-fetch",
            SERVICE,
            "--host",
            host,
            "--source-file",
            source,
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    fn env_show(&self, env_file: &str) -> Output {
        self.stado(&[
            "service",
            "env-show",
            SERVICE,
            "--host",
            "here",
            "--env-file",
            env_file,
        ])
    }

    /// A local destination outside the target home, so a written copy is never
    /// mistaken for the source it came from.
    fn destination(&self, name: &str) -> PathBuf {
        self.storage.path().join(name)
    }
}

/// This machine's own host name, normalized the way the registry stores it.
fn this_host() -> String {
    String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase()
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Everything the command said, for a refusal that may report on either
/// stream.
fn said(out: &Output) -> String {
    format!("{}{}", stdout(out), stderr(out))
}

/// The bytes on disk at `path`, which is what every claim here is judged
/// against.
fn on_disk(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The SHA-256 `shasum` would print, computed independently of the binary
/// under test.
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn report(out: &Output) -> serde_json::Value {
    let text = stdout(out);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("--json output is not JSON ({error}):\n{text}"));
    parsed
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or_else(|| panic!("--json output has no rows:\n{text}"))
}

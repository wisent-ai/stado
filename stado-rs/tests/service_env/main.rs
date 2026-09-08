//! `stado service env-show`, `env-set` and `endpoint-check` against a real
//! env file this machine holds, read and written by this machine's own tools.
//!
//! Every case drives the built `stado` binary. The registry target names THIS
//! machine in `hostnames` and declares no remote destination at all, so
//! `deploy/host_channel.rs::target_is_this_host` is true and the product takes
//! its current-host path: the program it would otherwise pipe to a login shell
//! is run here, against files this test made. There is no stand-in executable
//! on PATH and no channel to fake — the env file being read, the value being
//! written, the symlink being refused and the socket being reconciled are all
//! real local state, and `HOME` is a tempdir so the operator's own
//! `~/.config` can never be reached.
//!
//! Every success case is judged against the file on disk: the head line
//! reports the file's real mode and its real byte count, and the table's
//! VALUE cell is compared with the value the file's own bytes hold. A reader
//! that invented a value, or that rewrote the file it read, fails here.
//!
//! What is defended: a duplicate key is reported twice in file order with the
//! winner named; a credential-shaped value never leaves the host while an
//! endpoint-shaped one does whatever its key is called; `--reveal` opens
//! exactly one key; `env-set`'s write lands in the file and is read back;
//! and the refusals — a file that is not there, a host outside the registry,
//! a malformed key, a symlink, a path outside the target home and a value the
//! product declines — carry the sentences a live run printed.

mod endpoints;
mod refusals;
mod show;
mod write;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The service label every test addresses. Declared on the target itself, the
/// way `deploy/service.rs::declared_services` reads a registry-managed unit.
const SERVICE: &str = "com.wisent.always-on.weles";

/// Owner-only, the mode `env-set` requires of a value file and leaves on an
/// env file it writes.
const OWNER_ONLY: u32 = 0o600;

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

    /// Write the env file the tests read, and return its absolute path.
    fn env_file(&self, body: &str) -> PathBuf {
        let directory = self.home.path().join(".config/weles");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("worker.env");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        set_mode(&path, OWNER_ONLY);
        path
    }

    /// A local file holding one value, in the shape `env-set` demands.
    fn value_file(&self, name: &str, value: &str, mode: u32) -> PathBuf {
        let path = self.storage.path().join(name);
        std::fs::write(&path, value).unwrap();
        set_mode(&path, mode);
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

    fn env_show(&self, env_file: &str, extra: &[&str]) -> Output {
        self.service("env-show", "here", env_file, extra)
    }

    fn endpoint_check(&self, env_file: &str, extra: &[&str]) -> Output {
        self.service("endpoint-check", "here", env_file, extra)
    }

    /// One of the two env-file readers, against any host name — the refusals
    /// need a name the registry does not hold.
    fn service(&self, verb: &str, host: &str, env_file: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "service",
            verb,
            SERVICE,
            "--host",
            host,
            "--env-file",
            env_file,
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    /// `env-set` one key from an owner-only value file this call writes.
    fn env_set(&self, key: &str, env_file: &str, value: &str) -> Output {
        let path = self.value_file(&format!("value-{key}"), value, OWNER_ONLY);
        self.env_set_from(key, env_file, path.to_str().unwrap())
    }

    /// `env-set` from a value file the caller controls, so a refused value
    /// file can be handed over exactly as it sits on disk.
    fn env_set_from(&self, key: &str, env_file: &str, value_file: &str) -> Output {
        self.stado(&[
            "service",
            "env-set",
            SERVICE,
            "--host",
            "here",
            "--key",
            key,
            "--env-file",
            env_file,
            "--value-file",
            value_file,
        ])
    }
}

/// This machine's own host name, normalized the way the registry stores it.
fn this_host() -> String {
    String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase()
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
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

/// The file as it stands on disk right now.
fn on_disk(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {} back: {error}", path.display()))
}

/// The head line `env file: … (mode 600, owner-only, N bytes)`, asserted
/// against the bytes the file really holds. A report whose length disagrees
/// with the file did not read this file.
fn assert_head_matches_disk(text: &str, path: &Path) {
    let body = on_disk(path);
    let head = text
        .lines()
        .find(|line| line.starts_with("env file:"))
        .unwrap_or_else(|| panic!("no env file head in:\n{text}"));
    assert!(
        head.contains(path.to_str().unwrap()),
        "the head names another file:\n{head}"
    );
    assert!(
        head.contains("mode 600, owner-only"),
        "the file's real protection is not reported:\n{head}"
    );
    assert!(
        head.contains(&format!("{} bytes", body.len())),
        "the reported size is not this file's {} bytes:\n{head}",
        body.len()
    );
}

/// The value the shell would end up with for `key`, taken from the file's own
/// bytes: the last assignment wins, in either spelling.
fn effective_on_disk(path: &Path, key: &str) -> String {
    let body = on_disk(path);
    body.lines()
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line);
            line.strip_prefix(&format!("{key}="))
        })
        .last()
        .unwrap_or_else(|| panic!("{key} is assigned nowhere in {}", path.display()))
        .trim_matches('\'')
        .to_string()
}

/// The row for one key, as the printed table spells it.
///
/// Columns: LINE FORM KEY RESOLUTION VALUE STATE CHARS VALUE.
fn row<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find(|line| line.split_whitespace().nth(2) == Some(key))
        .unwrap_or_else(|| panic!("no table row for {key} in:\n{text}"))
}

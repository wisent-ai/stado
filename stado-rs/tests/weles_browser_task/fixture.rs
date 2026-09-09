//! One isolated fleet: a registry naming THIS machine, a real allowlist file
//! in a tempdir home, and the built binary run against both.
//!
//! No host name here is invented. The registry row's `hostnames` carries the
//! kernel host name of the machine running the test, read from `/bin/hostname`,
//! which is what makes [`crate::fixture::Fleet`] a local target: the product
//! reaches it without ssh, runs the fetch script through `/bin/bash -s`, and
//! reads a file this test actually wrote. There is no ssh destination on the
//! row at all, so no case here can reach the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The registry name of the one target every case places against.
pub const TARGET: &str = "here";

/// The action a plan runs as when it names none, and the two neighbours the
/// host's allowlist carries so a refusal has something to recommend.
pub const DEFAULT_ACTION: &str = "generic_browser_task";
pub const SAVED_ACTION: &str = "generic_saved_task";
pub const LOGIN_ACTION: &str = "apple_login";

/// The action `host weles-capture` hard-codes, and the one every refusal case
/// asks for: a host that does not carry it must say so rather than enqueue.
pub const CAPTURE_ACTION: &str = "generic_capture";

/// The plan schema `stado-rs/data/workloads.json` declares for this workload.
pub const PLAN_SCHEMA: &str = "wisent.weles-browser-task-plan.v1";

/// The recording label every plan carries, so a case can prove no run was
/// enqueued by finding nothing on disk that names it.
pub const SESSION_LABEL: &str = "weles-browser-task-area";

/// The catalog the worker reads its gate out of, relative to the target home.
pub const ALLOWLIST_PATH: &str = "weles/scripts/worker/deploy/weles-action-allowlist.txt";

/// This machine, as the kernel names it.
pub fn this_machine() -> String {
    let out = Command::new("/bin/hostname")
        .output()
        .expect("/bin/hostname runs");
    String::from_utf8(out.stdout)
        .expect("the kernel host name is UTF-8")
        .trim()
        .to_ascii_lowercase()
}

pub struct Fleet {
    home: tempfile::TempDir,
    store: tempfile::TempDir,
}

impl Fleet {
    /// A registry whose single row is this machine, declaring `actions` for
    /// Weles. Placement admits a plan whose action is one of them.
    pub fn declaring(actions: &[&str]) -> Self {
        Self::seed(TARGET, Some(json!({ "enabled": true, "actions": actions })))
    }

    /// A registry whose single row is this machine and carries no `weles` key
    /// at all: the host is real, and it declares no Weles work.
    pub fn without_weles(name: &str) -> Self {
        Self::seed(name, None)
    }

    fn seed(name: &str, weles: Option<Value>) -> Self {
        let fleet = Self {
            home: tempfile::tempdir().expect("temp home"),
            store: tempfile::tempdir().expect("temp store"),
        };
        let mut target = json!({
            "name": name,
            "kind": "local",
            "release_platform": platform(),
            "hostnames": [this_machine()],
            "role": "interactive",
            "services": [],
        });
        if let Some(policy) = weles {
            target["weles"] = policy;
        }
        let registry = json!({
            "schema_version": 2,
            "targets": [target],
            "coordinators": [],
        });
        std::fs::write(
            fleet.store.path().join("registry.json"),
            serde_json::to_string_pretty(&registry).expect("registry document"),
        )
        .expect("seed registry");
        fleet
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Write the worker's action catalog, one action per line unless the case
    /// is about the legacy assignment form.
    pub fn allowlist(&self, body: &str) {
        let path = self.home.path().join(ALLOWLIST_PATH);
        std::fs::create_dir_all(path.parent().expect("the catalog has a parent"))
            .expect("catalog directory");
        std::fs::write(&path, body).expect("write the catalog");
    }

    /// A plan document carrying the declared schema, the three fields every
    /// plan needs, and whatever else the case declares.
    pub fn plan(&self, extra: Value) -> PathBuf {
        let mut document = json!({
            "schema": PLAN_SCHEMA,
            "url": "https://example.com/",
            "objective": "read the page and report the outcome",
            "session_label": SESSION_LABEL,
        });
        let object = document.as_object_mut().expect("the plan is an object");
        for (key, value) in extra.as_object().expect("the extra fields are an object") {
            object.insert(key.clone(), value.clone());
        }
        let path = self.home.path().join("plan.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&document).expect("plan document"),
        )
        .expect("write the plan");
        path
    }

    /// The built binary, under this fleet's home and store and nothing else.
    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("NO_COLOR", "1")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store.path())
            .env(
                "STADO_CONFIG",
                self.store.path().join("no-such-config.json"),
            )
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            .output()
            .expect("the built stado binary runs")
    }

    /// `stado workload run weles-browser-task` against this fleet's one row.
    pub fn run(&self, target: Option<&str>, plan: &Path) -> Output {
        let plan = plan.to_string_lossy().into_owned();
        let mut args = vec!["workload", "run", "weles-browser-task"];
        if let Some(name) = target {
            args.push("--target");
            args.push(name);
        }
        args.push("--plan");
        args.push(&plan);
        self.stado(&args)
    }

    /// Every file under this fleet's home or store whose bytes name the
    /// session label. A refusal that enqueued nothing leaves none.
    pub fn files_naming_the_session(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for root in [self.home.path(), self.store.path()] {
            collect_mentions(root, &mut found);
        }
        found
    }
}

fn collect_mentions(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // The interpreter the fetch script uses caches bytecode under the
        // isolated home; that tree is macOS's, not this command's.
        if path.ends_with("Library") {
            continue;
        }
        if path.is_dir() {
            collect_mentions(&path, found);
            continue;
        }
        if path.file_name().is_some_and(|name| name == "plan.json") {
            continue;
        }
        if std::fs::read(&path)
            .is_ok_and(|bytes| find_bytes(&bytes, SESSION_LABEL.as_bytes()).is_some())
        {
            found.push(path);
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

/// Everything the command said, in the order an operator reads it.
pub fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The `Error:` line the CLI prints, without the tracing report under it.
pub fn refusal(out: &Output) -> String {
    let text = said(out);
    text.lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .unwrap_or_else(|| panic!("no refusal line in:\n{text}"))
        .to_string()
}

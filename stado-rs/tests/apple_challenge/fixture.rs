//! The isolated canonical registry these cases judge, the product invocation
//! that reads it, and the readers that say whether anything moved.
//!
//! Every invocation gets its own `HOME`, a `STADO_CONFIG` at a path that does
//! not exist, and a local store holding one registry document this file wrote.
//! No canonical registry, no operator config and no host is touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use stado::targets::REGISTRY_SCHEMA_VERSION;

/// The workload kind under judgement, and the declaration every refusal must
/// send the operator to.
pub(crate) const KIND: &str = "gui-automation";
pub(crate) const DECLARATION: &str = "stado-rs/data/workloads.json";

/// The plan schema the workload declares. A plan without it is refused before
/// any work is enqueued.
pub(crate) const PLAN_SCHEMA: &str = "wisent.gui-automation-plan.v1";

/// The two platforms these cases separate: the one the preparation is declared
/// for, and one that is not it.
pub(crate) const APPLE_PLATFORM: &str = "darwin-arm64";
pub(crate) const OTHER_PLATFORM: &str = "linux-amd64";

/// A declared Darwin ARM64 host reached over a destination RFC 2606 reserves,
/// so the case is about a host the product cannot reach and never about a
/// machine somebody owns.
pub(crate) const APPLE_HOST: &str = "apple-preparation-host";
/// A declared host of the other platform.
pub(crate) const OTHER_HOST: &str = "linux-only-host";
/// A name the seeded registry does not declare.
pub(crate) const UNDECLARED_HOST: &str = "no-such-apple-preparation-host";

/// One registry target, unreachable on purpose: the ssh host and the declared
/// hostnames differ so the registry accepts the document, and `.invalid` can
/// never resolve.
pub(crate) fn target(name: &str, platform: &str) -> Value {
    json!({
        "name": name,
        "kind": "local",
        "ssh": format!("nobody@{name}.invalid"),
        "release_platform": platform,
        "hostnames": [format!("{name}.local")],
    })
}

/// A registry document holding exactly the targets a case is about.
pub(crate) fn registry(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": targets,
        "coordinators": [],
    })
}

pub(crate) struct Fixture {
    home: tempfile::TempDir,
    store: PathBuf,
    registry: PathBuf,
    before: Vec<u8>,
}

impl Fixture {
    pub(crate) fn new(document: &Value) -> Self {
        let home = tempfile::tempdir().expect("an isolated home exists");
        let store = home.path().join("store");
        std::fs::create_dir_all(&store).expect("the isolated store exists");
        let registry = store.join("registry.json");
        let bytes = format!(
            "{}\n",
            serde_json::to_string_pretty(document).expect("serialize the registry")
        );
        std::fs::write(&registry, &bytes).expect("seed the isolated registry");
        let before = std::fs::read(&registry).expect("read the seeded registry");
        Self {
            home,
            store,
            registry,
            before,
        }
    }

    /// The built binary against this fixture's own store, with styling off so a
    /// refusal sentence arrives as text and not as escape sequences.
    pub(crate) fn stado(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env("HOME", self.home.path())
            .env("STADO_CONFIG", self.home.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.store)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            .env("NO_COLOR", "1")
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .output()
            .expect("the built Stado binary starts")
    }

    /// One plan document on disk, which is what `--plan` reads.
    pub(crate) fn plan(&self, document: &Value) -> PathBuf {
        let path = self.home.path().join("plan.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(document).expect("serialize the plan"),
        )
        .expect("write the plan document");
        path
    }

    /// The registry exactly as the product left it.
    pub(crate) fn registry_unchanged(&self) -> bool {
        std::fs::read(&self.registry).expect("read the registry after the command") == self.before
    }

    /// The cache the GUI-automation reader creates on a host it actually
    /// reached. Its absence is how "nothing was prepared" is read here.
    pub(crate) fn gui_cache(&self) -> PathBuf {
        self.home.path().join(".stado/cache/gui-automation")
    }

    pub(crate) fn home(&self) -> &Path {
        self.home.path()
    }
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub(crate) fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output),
    )
}

/// The one JSON report a `--json` GUI-automation command printed.
pub(crate) fn report(output: &Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|error| panic!("expected one JSON report ({error}):\n{}", said(output)))
}

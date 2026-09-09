//! The isolated store, the product invocation and the cache readers shared by
//! the registry-cache refusal cases.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.
//!
//! The store is the loopback object gateway in [`crate::gateway`], not a
//! directory on this disk: since `c637b026` a local filesystem store neither
//! records nor reads the last-known-good copy, so a local store cannot
//! observe the behaviour these cases are about.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The registry name of the host every case declares. It carries this
/// machine's own kernel host name, so the product takes its current-host path.
pub const TARGET: &str = "cache-observation";

/// The two files the reader-side cache is made of, named the way
/// `targets::last_good` names them.
pub const CACHE_DOCUMENT: &str = "registry-last-good.json";
pub const CACHE_SIDECAR: &str = "registry-last-good.meta.json";

/// A cleaner key no release of this build implements. `validate_registry`
/// refuses the whole document over it, which is the condition these cases are
/// about: a cache asked to record a document this build does not accept.
pub const UNIMPLEMENTED_CLEANER_KEY: &str = "keep_oldest";

/// The refusal's own opening, printed by the cache for every refusal that has
/// a path to name.
pub const REFUSAL_PREFIX: &str = "[registry-cache] not recording the last-known-good registry in";

/// The validator's own words for [`declares_an_unimplemented_key`], copied
/// from a live run of the built binary against that document.
pub fn contract_refusal() -> String {
    format!(
        "registry.targets[0].disk_cleanup.cleaners.build_caches: \
         unknown keys ['{UNIMPLEMENTED_CLEANER_KEY}']"
    )
}

pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The kernel's own host name, normalized the way the registry validator
/// requires: a declared name that is not lower-cased is refused outright.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

/// One host, this machine, with the full `disk_cleanup` field set and one
/// cleaner this build implements. `validate_registry` accepts it, which is
/// what makes every refusal below attributable to the mutation that caused it.
pub fn accepted_document() -> String {
    format!(
        r#"{{
  "schema_version": 2,
  "coordinators": [],
  "targets": [
    {{
      "name": "{TARGET}",
      "kind": "local",
      "ssh": null,
      "release_platform": "{}",
      "hostnames": ["{}"],
      "services": [],
      "disk_cleanup": {{
        "mode": "report",
        "check_interval_seconds": 3600,
        "low_free_gb": 100,
        "target_free_gb": 200,
        "max_bytes_per_pass": 68719476736,
        "max_items_per_pass": 512,
        "max_scan_items": 4096,
        "cleaners": {{ "build_caches": {{ "min_age_seconds": 86400 }} }}
      }}
    }}
  ]
}}
"#,
        platform(),
        hostname(),
    )
}

/// The accepted document with one cleaner key this build has no field for.
/// The loader still models the host — which is the point: a document the
/// reader tolerates and the contract refuses is exactly what the cache gate
/// exists to catch.
pub fn declares_an_unimplemented_key() -> String {
    accepted_document().replace(
        r#""min_age_seconds": 86400"#,
        &format!(r#""min_age_seconds": 86400, "{UNIMPLEMENTED_CLEANER_KEY}": 3"#),
    )
}

/// Schema-valid and names no hosts: what the authority served for nine minutes
/// on 2026-08-31.
pub fn names_no_hosts() -> String {
    "{\"schema_version\": 2, \"coordinators\": [], \"targets\": []}\n".to_string()
}

/// The namespace the store answers under. Lowercase letters, digits and `-`
/// only: the client refuses anything else before a request leaves.
pub const NAMESPACE: &str = "cache-observation";

pub struct Fixture {
    root: tempfile::TempDir,
    gateway: crate::gateway::Gateway,
}

impl Fixture {
    /// An isolated storage root holding the accepted document, and a `HOME`
    /// inside it so the cache this fleet's operator carries is never touched.
    pub fn new() -> Self {
        let fixture = Self::empty();
        fixture.publish(&accepted_document());
        fixture
    }

    /// The same root with the store answering `404` for the registry: nobody
    /// has published one, which is one of the states these cases put it in.
    pub fn empty() -> Self {
        let root = tempfile::tempdir().expect("an isolated storage root");
        std::fs::create_dir_all(root.path().join("home")).expect("a temporary HOME");
        let token = root.path().join("storage-token");
        std::fs::write(&token, "cache-observation-token").expect("write the store's bearer");
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))
            .expect("the bearer is owner-only, which the client requires");
        Self {
            root,
            gateway: crate::gateway::Gateway::start(),
        }
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// Serve `document` as the canonical registry, byte for byte. This is the
    /// authority for every command below, and it answers over TCP.
    pub fn publish(&self, document: &str) -> String {
        self.gateway.serve(document)
    }

    /// Leave the authority up but unable to answer, so the reader has to serve
    /// this host's recorded copy instead.
    pub fn withdraw(&self) {
        self.gateway.set(crate::gateway::Answer::Unavailable);
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        self.command(args)
            .env("HOME", self.home())
            .output()
            .expect("the built stado binary runs")
    }

    /// The same invocation with no `HOME` at all: a process with no cache
    /// location, which the product has to survive rather than refuse.
    pub fn stado_without_home(&self, args: &[&str]) -> Output {
        self.command(args)
            .env_remove("HOME")
            .output()
            .expect("the built stado binary runs")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", self.gateway.origin())
            .env(
                "WC_STADO_STORAGE_TOKEN_FILE",
                self.path().join("storage-token"),
            )
            .env("WC_STADO_STORAGE_NAMESPACE", NAMESPACE)
            .env_remove("WC_LOCAL_STORAGE_PATH")
            .env("STADO_CONFIG", self.path().join("no-such-config.json"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR");
        command
    }

    pub fn cache_directory(&self) -> PathBuf {
        self.home().join(".stado").join("cache")
    }

    pub fn cache_document(&self) -> PathBuf {
        self.cache_directory().join(CACHE_DOCUMENT)
    }

    pub fn cache_sidecar(&self) -> PathBuf {
        self.cache_directory().join(CACHE_SIDECAR)
    }

    /// The recorded copy as bytes, or `None` when this host has recorded none.
    pub fn recorded_copy(&self) -> Option<String> {
        std::fs::read_to_string(self.cache_document()).ok()
    }

    /// The generation the sidecar says the recorded copy is.
    pub fn recorded_generation(&self) -> String {
        let sidecar = std::fs::read_to_string(self.cache_sidecar()).expect("the sidecar is there");
        let parsed: Value = serde_json::from_str(&sidecar).expect("the sidecar is JSON");
        parsed["generation"]
            .as_str()
            .expect("the sidecar names the generation")
            .to_string()
    }

    /// Drive one read of the registry through the cheapest command that
    /// performs one: `registry self` resolves this machine against the
    /// canonical document and takes the same authority-then-copy path every
    /// host-addressed command takes.
    pub fn read_registry(&self) -> Output {
        self.stado(&["registry", "self"])
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

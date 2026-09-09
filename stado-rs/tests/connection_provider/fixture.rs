//! The product invocation and the readers the connection-path cases share.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself: the `HOME` override below is what
//! pushed the area past it.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use crate::owned_home;

/// One product invocation against the seeded store at `storage`.
///
/// `HOME` is a directory this test owns in every case, the real host journey
/// included: the product records a last-known-good registry copy beneath
/// `HOME`, and that copy is the operator's state rather than anything this
/// area is about.
///
/// `isolated_config` says whether the case reads a configuration at all. The
/// real host journey does — its host key is brokered through the operator's
/// configuration — so that one gets the operator's configuration and ssh
/// identity copied into the owned home instead of a home that borrows them.
pub fn stado(storage: &Path, isolated_config: bool, args: &[&str]) -> Output {
    let home = storage.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("HOME", &home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    if isolated_config {
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        command.env("STADO_CONFIG", storage.join("no-such-config.json"));
    } else {
        owned_home::copy_ssh_identity(&home);
        owned_home::copy_stado_config(&home);
    }
    command.output().expect("stado binary runs")
}

/// `stado registry pull`, read against the canonical registry the operator's
/// configuration names.
///
/// The journey needs the canonical document, so the configuration and the ssh
/// identity are copied into a home this call owns. Reading the operator's
/// registry is the point; recording a copy of it into the operator's
/// `~/.stado/cache` is not, and an owned `HOME` is what keeps the two apart.
pub fn pull_canonical() -> Output {
    let home = tempfile::tempdir().expect("a home this journey owns");
    owned_home::copy_ssh_identity(home.path());
    owned_home::copy_stado_config(home.path());
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["registry", "pull"])
        .env("HOME", home.path())
        .output()
        .expect("stado registry pull runs")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn document(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document, got {error}\nstdout: {}\nstderr: {}",
            stdout(output),
            stderr(output)
        )
    })
}

pub fn seed(registry: &Value) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("registry.json"),
        format!("{}\n", serde_json::to_string_pretty(registry).unwrap()),
    )
    .unwrap();
    directory
}

/// This machine's hostname, which is what the product matches a local target
/// against.
pub fn this_hostname() -> String {
    let output = Command::new("hostname").output().expect("hostname runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

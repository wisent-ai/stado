//! The isolated store declaration, and the real `stado` binary run against
//! it.
//!
//! One `TempDir` per case holds the bearer file and the HOME the child is
//! given. The child's environment is cleared and rebuilt from nothing, so no
//! operator credential, proxy, profile or configuration file can reach a
//! case, and `STADO_CONFIG` names a path that does not exist, which is how
//! this build disables configuration-file discovery.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use crate::gateway::Gateway;

/// The object namespace the store is declared with. Any name the object
/// reference accepts would do; this one is the deployment's own.
pub const NAMESPACE: &str = "probierz";

/// The storage backend id that selects `queue::stado_object::StadoObjectBackend`
/// — the reader every case here exercises.
pub const BACKEND: &str = "stado";

/// One isolated store, declared at a gateway.
pub struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("an isolated root");
        std::fs::create_dir_all(dir.path().join("home")).expect("the isolated root is writable");
        let token = dir.path().join("bearer");
        std::fs::write(&token, "truncation-area-bearer").expect("the bearer file is writable");
        // The constructor refuses a bearer any other account can read, so the
        // mode is part of declaring the store at all.
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))
            .expect("the bearer file takes an owner-only mode");
        Self { dir }
    }

    pub fn token_file(&self) -> PathBuf {
        self.dir.path().join("bearer")
    }

    /// The bearer the product should present, as it was written.
    pub fn token(&self) -> String {
        std::fs::read_to_string(self.token_file()).expect("the bearer file is readable")
    }

    /// Run the real `stado` binary with the gateway declared as its store.
    pub fn stado(&self, gateway: &Gateway, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path().join("home"))
            // Set but missing: configuration-file discovery is off, so the
            // operator's real configuration cannot reach this child.
            .env("STADO_CONFIG", self.dir.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", BACKEND)
            .env("WC_STADO_STORAGE_URL", gateway.origin())
            .env("WC_STADO_STORAGE_TOKEN_FILE", self.token_file())
            .env("WC_STADO_STORAGE_NAMESPACE", NAMESPACE)
            .output()
            .expect("the built stado binary runs")
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

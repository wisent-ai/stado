//! The isolated store this area declares, and how the binary is run against it.
//!
//! A tempdir per case holds the owner-only bearer file the object client
//! requires, the destination a download would be written to, and the `HOME`
//! and `STADO_CONFIG` that keep the operator's own vault, registry and
//! configuration out of reach. `WC_STORAGE_BACKEND=stado` with
//! `WC_STADO_STORAGE_URL` pointing at the case's own gateway is the whole
//! declaration: from there the product resolves its object routes to a socket
//! this test owns.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

use crate::gateway::Gateway;

/// The fixed PATH the binary under test is given.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The namespace declared as the store's own, and a key inside it. A bare
/// path — one without a `stado://` scheme — addresses this namespace, which is
/// the form `stado storage stat` documents for a queue object.
pub const NAMESPACE: &str = "queue";
pub const OBJECT_KEY: &str = "registry.json";

/// Owner-only, as `StadoObjectBackend::new` requires: a bearer readable by
/// group or other is refused before any request is sent.
const OWNER_ONLY: u32 = 0o600;
const BEARER: &str = "object-auth-verdict-fixture-token";

/// An isolated store declaration bound to one gateway.
pub struct Store {
    _dir: tempfile::TempDir,
    pub gateway: Gateway,
    root: PathBuf,
    home: PathBuf,
    token_file: PathBuf,
}

impl Store {
    /// A store whose gateway answers `status` for every object route.
    pub fn answering(status: u16) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-object-auth-")
            .tempdir()
            .expect("create the isolated store");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create the isolated home");
        let token_file = root.join("storage-token");
        fs::write(&token_file, BEARER).expect("write the fixture bearer");
        fs::set_permissions(&token_file, fs::Permissions::from_mode(OWNER_ONLY))
            .expect("make the fixture bearer owner-only");
        Self {
            _dir: dir,
            gateway: Gateway::answering(status),
            root,
            home,
            token_file,
        }
    }

    /// Run the built binary against this store and nothing else.
    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", &self.root)
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", self.gateway.url())
            .env("WC_STADO_STORAGE_TOKEN_FILE", &self.token_file)
            .env("WC_STADO_STORAGE_NAMESPACE", NAMESPACE)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .output()
            .expect("the built stado binary did not start")
    }

    /// Where a download would land. Never created by the fixture: a case
    /// proves a refused read wrote nothing by finding it absent.
    pub fn destination(&self) -> PathBuf {
        self.root.join("downloaded.bin")
    }

    /// The `stado://` form of the object under test.
    pub fn uri(&self) -> String {
        format!("stado://{NAMESPACE}/{OBJECT_KEY}")
    }
}

/// One JSON document a command printed on stdout.
pub fn printed(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "the command did not print one JSON document: {error}\n{}",
            said(&output.stdout)
        )
    })
}

pub fn said(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

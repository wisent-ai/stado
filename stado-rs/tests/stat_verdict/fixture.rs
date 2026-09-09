//! The two stores these cases ask, and the readers of what the command said.
//!
//! Both stores are real and both are this machine's: one is a directory the
//! test creates and writes objects into, the other is the loopback listener in
//! [`crate::object_api`]. The binary under test is the built `stado`
//! (`CARGO_BIN_EXE_stado`), run with an emptied environment so nothing the
//! developer has configured can decide what a case measures.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The object namespace the loopback store is addressed in. A declared name,
/// not a coordinate that exists anywhere: `ObjectRef` requires lowercase
/// letters, digits and `-`, and every request the listener sees carries it.
pub const NAMESPACE: &str = "stat-verdict";

/// The mode bits the product's own token-file check requires, and the ones it
/// refuses. Copied from `StadoObjectBackend::new`, which reads
/// `mode() & 0o077` and refuses anything a group or the world can read.
pub const OWNER_ONLY: u32 = 0o600;
pub const WORLD_READABLE: u32 = 0o644;

/// The tools that answer for this machine rather than for the test: the
/// digest of a file as the system computes it.
const SHASUM: &str = "/usr/bin/shasum";

/// Run the built binary with nothing inherited but the two variables a
/// subprocess cannot work without, plus the store settings the case names.
///
/// `STADO_CONFIG` points at a path that does not exist, so a configuration
/// file can never supply a store the case did not ask for.
///
/// `NO_COLOR` is the product's own switch for the unstyled rendering: the
/// classified failure line is written for a terminal, and with styling on,
/// `error_code` and `="auth"` are separated by escape sequences, so the field
/// a log shipper ingests is not the field a reader sees. The cases assert the
/// text an operator's log line carries.
fn run(home: &Path, settings: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .env("STADO_CONFIG", home.join("no-such-config.json"));
    for (name, value) in settings {
        command.env(name, value);
    }
    command
        .args(args)
        .output()
        .expect("the built stado binary runs")
}

/// A filesystem store rooted in this test's own directory.
pub struct LocalStore {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl LocalStore {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("a store root of this test's own"),
            home: tempfile::tempdir().expect("a home of this test's own"),
        }
    }

    /// Where the store keeps the object named `name`. The local backend stores
    /// a blob under the name it is handed, so this is the file the command
    /// will read.
    pub fn object_path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// Put bytes in the store the way any other writer would, and hand back
    /// the path they landed at so a case can read them again.
    pub fn write_object(&self, name: &str, bytes: &str) -> PathBuf {
        let path = self.object_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the object's directory exists");
        }
        std::fs::write(&path, bytes).expect("write the object into the store");
        path
    }

    pub fn stat(&self, args: &[&str]) -> Output {
        let root = self.root.path().to_string_lossy().into_owned();
        let mut full = vec!["storage", "stat"];
        full.extend_from_slice(args);
        run(
            self.home.path(),
            &[
                ("WC_STORAGE_BACKEND", "local"),
                ("WC_LOCAL_STORAGE_PATH", &root),
            ],
            &full,
        )
    }
}

/// The object-API store: a token file this test owns, and whichever loopback
/// URL the case points it at.
pub struct ObjectStore {
    home: tempfile::TempDir,
    token: PathBuf,
}

impl ObjectStore {
    /// A store whose token file carries exactly `mode`, so a case can ask the
    /// product about a credential this machine really refuses.
    pub fn with_token_mode(mode: u32) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().expect("a home of this test's own");
        let token = home.path().join("object-api.token");
        std::fs::write(&token, "the bearer this test wrote").expect("write the token file");
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(mode))
            .expect("set the token file's mode");
        Self { home, token }
    }

    pub fn token(&self) -> &Path {
        &self.token
    }

    /// The mode the token file carries on disk right now, so a case asserts
    /// against the filesystem rather than against its own intention.
    pub fn token_mode(&self) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(&self.token)
            .expect("the token file this test wrote is on disk")
            .permissions()
            .mode()
            & 0o777
    }

    pub fn stat(&self, url: &str, path: &str) -> Output {
        let token = self.token.to_string_lossy().into_owned();
        run(
            self.home.path(),
            &[
                ("WC_STORAGE_BACKEND", "stado"),
                ("WC_STADO_STORAGE_URL", url),
                ("WC_STADO_STORAGE_TOKEN_FILE", &token),
                ("WC_STADO_STORAGE_NAMESPACE", NAMESPACE),
            ],
            &["storage", "stat", path, "--json"],
        )
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Everything the command said, for a failure message that shows the run.
pub fn said(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

/// The receipt `--json` printed. A command that answered nothing prints no
/// receipt at all, which is why the cases that expect one read it here and the
/// cases that expect none assert on empty stdout instead.
pub fn receipt(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON receipt: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

pub fn state(output: &Output) -> String {
    receipt(output)["state"]
        .as_str()
        .unwrap_or_else(|| panic!("the receipt carries no state: {}", said(output)))
        .to_string()
}

/// The exit status as a number, so a case can name the code the fleet's retry
/// contract uses instead of only "not zero".
pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or_else(|| {
        panic!(
            "the command was killed rather than exiting: {}",
            said(output)
        )
    })
}

/// The SHA-256 of a file as this machine computes it, which is what the local
/// backend's version token is: `LocalBackend::version` digests the bytes it
/// read.
pub fn digest_on_disk(path: &Path) -> String {
    let output = Command::new(SHASUM)
        .args(["-a", "256"])
        .arg(path)
        .output()
        .expect("the system digest tool runs");
    assert!(
        output.status.success(),
        "the system could not digest {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("the digest tool printed a digest")
        .to_string()
}

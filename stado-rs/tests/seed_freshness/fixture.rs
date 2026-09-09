//! The isolated host these cases ask about: an isolated registry naming this
//! machine, a `HOME` inside the storage root, a real Skarbiec vault created
//! and filled through the released Skarbiec this host carries, and the
//! `stado credentials seed-freshness` invocation itself.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The registry name of the host every case asks about. It carries this
/// machine's own kernel host name, so both host reads happen here.
pub const TARGET: &str = "seed-observation";

/// The released Skarbiec this machine carries, relative to the real `HOME`.
/// The command resolves the vault reader at `$HOME/.stado/bin/skarbiec` on the
/// host it is asking, so the fixture links this binary into its own `HOME` and
/// the read is performed by the same program a host performs it with.
pub const SKARBIEC_RELATIVE_PATH: &str = ".stado/bin/skarbiec";

/// The account the six-day sign-in loop locked out.
pub const LOCKED_ITEM: &str = "codex-wisent-google-sso";
/// A second account whose codes the provider accepted, so no case can pass by
/// reporting one verdict for everything.
pub const HEALTHY_ITEM: &str = "claude-wisent-google-sso";
/// A third that declares the field and carries nothing in it.
pub const EMPTY_FIELD_ITEM: &str = "codex-zuzanna-google-sso";

/// A Base32 seed the vault accepts. It is stored in this fixture's own vault
/// and never leaves it; the safety case proves the report does not carry it.
pub const STORED_SEED: &str = "JBSWY3DPEHPK3PXP";
pub const OTHER_SEED: &str = "JBSWY3DPEHPK3PXQ";

pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The kernel's own host name, lower-cased the way the registry validator
/// requires a declared name to be.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

/// The released Skarbiec installed on this machine, which the fixture's host
/// uses to answer the vault half.
pub fn installed_skarbiec() -> PathBuf {
    let home = std::env::var("HOME").expect("the test process has a home");
    Path::new(&home).join(SKARBIEC_RELATIVE_PATH)
}

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// A host declaring this machine, with its own vault, its own GnuPG home
    /// and this build installed where the command looks for a host's Stado.
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated storage root");
        let fixture = Self { root };
        for directory in [
            fixture.home().join(".stado/bin"),
            fixture.home().join(".config/stado"),
            fixture.home().join(".brama"),
        ] {
            std::fs::create_dir_all(&directory).expect("an isolated host directory");
        }
        std::fs::create_dir_all(fixture.gnupg_home()).expect("an isolated GnuPG home");
        std::fs::set_permissions(fixture.gnupg_home(), std::fs::Permissions::from_mode(0o700))
            .expect("GnuPG refuses a world-readable home");
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_stado"),
            fixture.home().join(".stado/bin/stado"),
        )
        .expect("install this build where the command reads a host's configuration");
        std::os::unix::fs::symlink(
            installed_skarbiec(),
            fixture.home().join(SKARBIEC_RELATIVE_PATH),
        )
        .expect("install this machine's released Skarbiec as the host's vault reader");
        std::fs::write(
            fixture.home().join(".config/stado/config.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "secrets": {"skarbiec": {"vault_file": fixture.vault().display().to_string()}}
            }))
            .expect("the host configuration serialises"),
        )
        .expect("declare the vault this host's credential operations resolve to");
        let registry = serde_json::json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "ssh": null,
                "release_platform": platform(),
                "hostnames": [hostname()],
                "services": [],
            }],
        });
        std::fs::write(
            fixture.path().join("registry.json"),
            serde_json::to_vec_pretty(&registry).expect("registry serialises"),
        )
        .expect("seed the isolated registry");
        fixture.skarbiec(&["init", "seed-observation-owner"]);
        fixture
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    pub fn vault(&self) -> PathBuf {
        self.home().join("vault.json")
    }

    pub fn gnupg_home(&self) -> PathBuf {
        self.home().join("gnupg")
    }

    pub fn journal(&self) -> PathBuf {
        self.home().join(".brama/journal.jsonl")
    }

    /// One read or write against this fixture's own vault, performed by the
    /// released Skarbiec.
    pub fn skarbiec(&self, args: &[&str]) -> Output {
        let output = Command::new(installed_skarbiec())
            .args(args)
            .env("SKARBIEC_VAULT_FILE", self.vault())
            .env("GNUPGHOME", self.gnupg_home())
            .env("HOME", self.home())
            .output()
            .expect("the released Skarbiec runs");
        assert!(
            output.status.success(),
            "skarbiec {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Store a login row carrying `seed`; an empty seed is a declared field
    /// with nothing in it, which is a different condition from a stale one.
    pub fn store_login(&self, item: &str, seed: &str) {
        self.skarbiec(&[
            "set",
            item,
            "--type",
            "login",
            &format!("username={item}@example.com"),
            "password=stored-only-in-this-fixture",
            &format!("totp_secret={seed}"),
        ]);
    }

    /// `stado credentials seed-freshness --host TARGET [--login-item ITEM]`,
    /// as JSON.
    pub fn freshness(&self, login_item: Option<&str>) -> (Value, Output) {
        let mut args = vec!["credentials", "seed-freshness", "--host", TARGET, "--json"];
        if let Some(item) = login_item {
            args.extend_from_slice(&["--login-item", item]);
        }
        let output = self.stado(&args);
        let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
                stdout(&output),
                stderr(&output)
            )
        });
        (report, output)
    }

    /// The same command without `--json`: the lines an operator reads.
    pub fn freshness_lines(&self) -> Output {
        self.stado(&["credentials", "seed-freshness", "--host", TARGET])
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.path())
            .env("STADO_CONFIG", self.path().join("no-such-config.json"))
            .env("HOME", self.home())
            .env("GNUPGHOME", self.gnupg_home())
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .env_remove("BRAMA_STATE_DIR")
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

/// The finding the report carries for `item`.
pub fn finding<'a>(report: &'a Value, item: &str) -> &'a Value {
    report["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("the report lists its findings: {report:#}"))
        .iter()
        .find(|finding| finding["login_item"] == serde_json::json!(item))
        .unwrap_or_else(|| panic!("no finding for {item}: {report:#}"))
}

/// Whether the report says anything at all about `item`.
pub fn mentions(report: &Value, item: &str) -> bool {
    report["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding["login_item"] == serde_json::json!(item))
    })
}

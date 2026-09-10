//! Two real Skarbiec binaries and one real GnuPG-backed vault per case.
//!
//! Nothing here is a stand-in. The vault is created by the broker itself with
//! the machine's own `gpg`, the item is a real encrypted login, and the
//! capability route is declared by the broker's own verb. What differs between
//! the two cases is only which real binary is installed at the path the
//! production code addresses, `$HOME/.stado/bin/skarbiec`:
//!
//! * [`current`] — a broker that knows the `route` verb group Skarbiec ships
//!   today, resolved through the shared support policy.
//! * [`stale`] — the genuine 0.2.39 source, built once in the same cache as
//!   the current broker. It still knows `routes add` and refuses `route`,
//!   regardless of which release the operator has since installed.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;

use super::fleet::Fleet;
use super::source::{historical_skarbiec, real_skarbiec};

pub const ITEM: &str = "route-real-login";
pub const FIELD: &str = "username";
pub const RESOURCE: &str = "origin:https://route.real.invalid/username";
const OWNER: &str = "Stado route tests <route-real@example.invalid>";
const REASON: &str = "stado route area real evidence";

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// True when this binary knows the verb group Skarbiec ships today. An older
/// broker answers `unknown command: route` and exits non-zero.
fn knows_route_group(binary: &Path) -> bool {
    Command::new(binary)
        .arg("route")
        .output()
        .unwrap_or_else(|error| panic!("{} could not start: {error}", binary.display()))
        .status
        .success()
}

/// The broker the fleet is supposed to be running: named in `SKARBIEC_BIN`, or
/// built from the sibling checkout at `origin/main`. Either way it is real,
/// and either way it has to know the verb group Stado reads with.
pub fn current() -> PathBuf {
    let binary = real_skarbiec();
    assert!(
        knows_route_group(&binary),
        "the resolved skarbiec at {} does not know the `route` verb group, so it cannot resolve a \
         declared route. `route resolve`, `route declare` and `route verify` replaced `routes \
         list`, `routes add` and `routes verify`, so this binary predates them. Unset SKARBIEC_BIN \
         to have this area build the sibling checkout at origin/main, or point it at a broker \
         built from there.",
        binary.display()
    );
    binary
}

/// A real pre-group broker, explicitly supplied or built from its pinned source.
pub fn stale() -> PathBuf {
    let binary = match std::env::var_os("SKARBIEC_STALE_BIN") {
        Some(configured) => PathBuf::from(configured),
        None => historical_skarbiec(),
    };
    assert!(
        executable(&binary),
        "no older skarbiec at {}: this case proves what Stado answers when a fleet host still runs \
         a broker without the `route` verb group, so it needs that real binary. Point \
         SKARBIEC_STALE_BIN at one.",
        binary.display()
    );
    assert!(
        !knows_route_group(&binary),
        "the broker at {} already knows the `route` verb group, so it cannot demonstrate the \
         delivery gap this case is about. Point SKARBIEC_STALE_BIN at a broker that predates it.",
        binary.display()
    );
    binary
}

/// One real vault on the isolated host, opened by one real broker.
pub struct Vault {
    /// gpg's agent socket lives here, and the system temp root is deep enough
    /// to overrun the socket path limit — the same reason `tests/support`
    /// keeps its GnuPG home under `~/.stado/work`.
    gnupg: tempfile::TempDir,
    binary: PathBuf,
    vault: PathBuf,
    /// Where the broker keeps the capability route table for this host. Both
    /// the fixture and the product reach it through the same `HOME`, so
    /// neither names it and neither can disagree about it.
    pub table: PathBuf,
}

impl Vault {
    /// Install `binary` where the production code looks for it, create the
    /// vault, seed one real item, and declare the capability route with the
    /// broker's own verb — `route declare` on a current broker, `routes add`
    /// on the older one, so the only thing the stale case is missing is the
    /// verb group Stado reads with.
    pub fn install(fleet: &Fleet, binary: &Path) -> Self {
        let installed = fleet.home.join(".stado/bin/skarbiec");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::copy(binary, &installed).expect("install the real broker on the isolated host");
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();

        let scratch = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".stado/work");
        fs::create_dir_all(&scratch).unwrap();
        let gnupg = tempfile::Builder::new()
            .prefix("route-real-gpg-")
            .tempdir_in(scratch)
            .unwrap();
        fs::set_permissions(gnupg.path(), fs::Permissions::from_mode(0o700)).unwrap();

        let vault = Self {
            gnupg,
            binary: installed,
            vault: fleet.home.join(".stado/skarbiec.vault.json"),
            table: fleet.home.join(".stado/capability-routes.json"),
        };
        vault.run(&fleet.home, &["init", OWNER], None);
        vault.run(
            &fleet.home,
            &["set-json", ITEM, "--type", "login"],
            Some(
                &json!({
                    "schema": "skarbiec.item.v2",
                    "kind": "login",
                    "context": {"service": "stado-route-tests"},
                    "fields": {FIELD: "route-real-user", "password": "not-read-by-this-test"},
                })
                .to_string(),
            ),
        );
        let declare: &[&str] = if knows_route_group(&vault.binary) {
            &["route", "declare"]
        } else {
            &["routes", "add"]
        };
        let mut arguments = declare.to_vec();
        arguments.extend_from_slice(&[
            "--resource",
            RESOURCE,
            "--item",
            ITEM,
            "--field",
            FIELD,
            "--reason",
            REASON,
        ]);
        vault.run(&fleet.home, &arguments, None);
        assert!(
            vault.table.is_file(),
            "the real broker declared a route but wrote no table at {}",
            vault.table.display()
        );
        vault
    }

    /// `HOME` is the isolated host's, so the table and vault this fixture
    /// writes are exactly the ones the product will resolve for that host.
    fn run(&self, home: &Path, arguments: &[&str], stdin: Option<&str>) {
        let mut child = Command::new(&self.binary)
            .args(arguments)
            .env_clear()
            .env("HOME", home)
            .env("GNUPGHOME", self.gnupg.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("SKARBIEC_VAULT_FILE", &self.vault)
            .env("SKARBIEC_AUDIT_FILE", self.gnupg.path().join("audit.jsonl"))
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the real skarbiec binary runs");
        if let Some(body) = stdin {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(body.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "real Skarbiec refused `{}`: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    pub fn gnupg_home(&self) -> &Path {
        self.gnupg.path()
    }

    pub fn vault_file(&self) -> &Path {
        &self.vault
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .args([
                "--homedir",
                self.gnupg.path().to_str().unwrap(),
                "--kill",
                "gpg-agent",
            ])
            .output();
    }
}

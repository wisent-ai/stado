//! `stado service file-fetch` against real files under a real home.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so `deploy/host_channel.rs` runs the remote
//! script locally through the same `/bin/bash -s` the ssh branch asks the login
//! shell for — the script under test is byte-identical either way, and only the
//! hop disappears. HOME is a tempdir, so the file being copied, the symlink
//! being refused and the oversized file being declined are all real state this
//! test made, and the operator's own `~/.ssh` can never be reached.
//!
//! There is no fake channel here and no stubbed digest. The bytes make a real
//! round trip through base64 and a real `/bin/bash`, the host-side SHA-256 is
//! `shasum`'s, and the local one is this binary's.
//!
//! What is defended: a fetched file is byte-exact where `env-show` of the same
//! file is not — that difference is the whole reason this command exists; a
//! symlink and a path outside the target home are refused with their exact
//! sentences and nothing is written; a file past the transfer limit is refused
//! before it is read rather than truncated; and the `--json` report carries
//! both digests and never the content.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The service label every test addresses. Declared on the target itself, the
/// way `deploy/service.rs::declared_services` reads a registry-managed unit.
const SERVICE: &str = "com.wisent.always-on.weles";

/// The bytes `env-show` cannot report and this command must.
///
/// Shaped like the file that motivated the command:
/// `$HOME/.stado/bin/weles-release-cutover` is a shell script whose working
/// parts are a double-quoted `sed -E` program, a line continuation, and a tab.
/// `env-show`'s host-side sanitizer replaces every quote, every backslash and
/// every byte outside printable ASCII with `?`, so its report of this file
/// would not run.
const AWKWARD: &str = "#!/bin/bash\nset -euo pipefail\n\
                       /usr/bin/sed -E \"/^WC_SKARBIEC_URL=/d\" \\\n\
                       \t\"$HOME/.config/weles/worker.env\"\n\
                       # naïve — non-ASCII: café ✓\n";

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        let registry = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": "here",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform(),
                "hostnames": [hostname],
                "slots": 1,
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

    /// Write a file under the target home and return its absolute path.
    fn file(&self, relative: &str, body: &[u8], mode: u32) -> PathBuf {
        let path = self.home.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
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

    fn file_fetch(&self, source: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "service",
            "file-fetch",
            SERVICE,
            "--host",
            "here",
            "--source-file",
            source,
        ];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    fn env_show(&self, env_file: &str) -> Output {
        self.stado(&[
            "service",
            "env-show",
            SERVICE,
            "--host",
            "here",
            "--env-file",
            env_file,
        ])
    }

    /// A local destination outside the target home, so a written copy is never
    /// mistaken for the source it came from.
    fn destination(&self, name: &str) -> PathBuf {
        self.storage.path().join(name)
    }
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

/// The SHA-256 `shasum` would print, computed independently of the binary
/// under test.
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn report(out: &Output) -> serde_json::Value {
    let text = stdout(out);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("--json output is not JSON ({error}):\n{text}"));
    parsed
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or_else(|| panic!("--json output has no rows:\n{text}"))
}

#[test]
fn a_fetched_file_is_byte_exact_where_env_show_of_the_same_file_is_not() {
    let fleet = Fleet::new();
    // 0700: live operator tooling, exactly the mode
    // `$HOME/.stado/bin/weles-release-cutover` carries on charless-mac-mini.
    let source = fleet.file(".stado/bin/weles-release-cutover", AWKWARD.as_bytes(), 0o700);
    let destination = fleet.destination("weles-release-cutover");

    let fetched = fleet.file_fetch(
        "$HOME/.stado/bin/weles-release-cutover",
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(
        fetched.status.success(),
        "fetch failed: {}{}",
        stdout(&fetched),
        stderr(&fetched)
    );

    let copied = std::fs::read(&destination).unwrap();
    let original = std::fs::read(&source).unwrap();
    assert_eq!(
        copied, original,
        "the copy is not byte-identical to the source"
    );
    // The digest is in the report, and it is the digest of these exact bytes.
    let table = stdout(&fetched);
    assert!(table.contains(&sha256(&original)), "{table}");
    assert!(table.contains("verified"), "{table}");

    // The gap this command closes, demonstrated rather than asserted about:
    // `env-show` is the only other reader of a file under a managed home, and
    // its report of this same file cannot reproduce it. Its sanitizer replaces
    // the quotes, the backslash and the non-ASCII bytes with `?`.
    let shown = fleet.env_show("$HOME/.stado/bin/weles-release-cutover");
    let described = stdout(&shown);
    assert!(
        described.contains('?'),
        "env-show reported no substitution, so this file needed no byte-exact reader:\n{described}"
    );
    assert!(
        !described.contains("café"),
        "env-show returned the source bytes verbatim:\n{described}"
    );

    // The mode the operator has to know before committing a launcher.
    let json = fleet.file_fetch(
        "$HOME/.stado/bin/weles-release-cutover",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    let row = report(&json);
    assert_eq!(row["file_state"], "read");
    assert_eq!(row["mode"], "700");
    assert_eq!(row["owner_only"], true);
    assert_eq!(row["integrity"], "verified");
    assert_eq!(row["host_digest"], row["local_digest"]);
    assert_eq!(row["host_digest"], serde_json::json!(sha256(&original)));
    assert_eq!(row["bytes"], serde_json::json!(original.len()));
    assert_eq!(row["fetched_bytes"], serde_json::json!(original.len()));
    // The bytes are the destination file's business. A report an operator
    // pastes into a ticket must not be a second copy of the content.
    let text = serde_json::to_string(&row).unwrap();
    assert!(!text.contains("content"), "{text}");
    assert!(!text.contains("WC_SKARBIEC_URL"), "{text}");
}

#[test]
fn a_symlink_under_the_target_home_is_refused_without_being_followed() {
    let fleet = Fleet::new();
    // The exact escape the confinement exists for: a link inside the managed
    // area whose target is the account's private key.
    let secret = fleet.file(".ssh/id_ed25519", b"PRIVATE KEY MATERIAL\n", 0o600);
    let link = fleet.home.path().join(".stado/bin/innocent-looking");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let destination = fleet.destination("leaked");

    let out = fleet.file_fetch(
        "$HOME/.stado/bin/innocent-looking",
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(
        !out.status.success(),
        "a refused fetch must exit non-zero:\n{}",
        stdout(&out)
    );
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(said.contains("refused_symlink"), "{said}");
    assert!(said.contains("was not followed"), "{said}");
    assert!(
        !destination.exists(),
        "a refusal wrote {}",
        destination.display()
    );
    assert!(
        !said.contains("PRIVATE KEY MATERIAL"),
        "the link's target crossed the channel:\n{said}"
    );
}

#[test]
fn a_path_outside_the_target_home_is_refused_before_anything_is_read() {
    let fleet = Fleet::new();
    // A real file that really exists and really is not under this home.
    let outside = fleet.destination("outside.txt");
    std::fs::write(&outside, b"not yours\n").unwrap();
    let destination = fleet.destination("copied-outside");

    let out = fleet.file_fetch(
        outside.to_str().unwrap(),
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(said.contains("refused_outside_home"), "{said}");
    assert!(!destination.exists());
    assert!(!said.contains("not yours"), "{said}");
}

#[test]
fn a_missing_file_is_reported_as_missing_rather_than_written_as_empty() {
    let fleet = Fleet::new();
    let destination = fleet.destination("never-existed");
    let out = fleet.file_fetch(
        "$HOME/.stado/bin/never-existed",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let row = report(&out);
    assert_eq!(row["file_state"], "missing");
    assert_eq!(row["integrity"], "unverified");
    assert_eq!(row["dest_file"], "-");
    assert!(
        !destination.exists(),
        "a missing source produced {}",
        destination.display()
    );
}

#[test]
fn a_file_past_the_transfer_limit_is_refused_whole_rather_than_truncated() {
    let fleet = Fleet::new();
    // One byte over. A prefix would hash consistently at both ends, so a
    // command that truncated here would report `verified` for half a program.
    let oversized = vec![b'x'; 1_048_576 + 1];
    fleet.file(".stado/bin/too-big", &oversized, 0o700);
    let destination = fleet.destination("too-big");

    let out = fleet.file_fetch(
        "$HOME/.stado/bin/too-big",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let row = report(&out);
    assert_eq!(row["file_state"], "refused_too_large");
    assert_eq!(row["bytes"], serde_json::json!(oversized.len()));
    assert_eq!(row["host_digest"], "");
    assert!(!destination.exists());
}

#[test]
fn a_fetch_without_a_destination_reports_the_file_and_keeps_no_copy() {
    let fleet = Fleet::new();
    let source = fleet.file(".config/weles/worker.env", b"WC_SKARBIEC_URL='x'\n", 0o600);
    let out = fleet.file_fetch("$HOME/.config/weles/worker.env", &["--json"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let row = report(&out);
    assert_eq!(row["integrity"], "verified");
    assert_eq!(row["dest_file"], "-");
    assert_eq!(
        row["local_digest"],
        serde_json::json!(sha256(&std::fs::read(&source).unwrap()))
    );
}

#[test]
fn a_relative_destination_is_refused_before_the_host_is_contacted() {
    let fleet = Fleet::new();
    fleet.file(".stado/bin/thing", b"#!/bin/sh\n", 0o700);
    let out = fleet.file_fetch("$HOME/.stado/bin/thing", &["--dest-file", "thing"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--dest-file must be absolute"),
        "{}",
        stderr(&out)
    );
    assert!(
        !Path::new("thing").exists(),
        "a refused destination was written into the working directory"
    );
}

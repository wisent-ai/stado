//! Native-reader convergence against the init system that owns the process.
//!
//! This macOS story loads a uniquely named
//! LaunchAgent into the current login's real launchd domain and serves the real
//! `stado dashboard` on an isolated loopback port. The unit starts from a
//! private copy of the built Stado binary. While that process is still live, the
//! plist is changed to name the delivered `$HOME/.stado/bin/stado`, reproducing
//! the state in which launchd retains an old cached definition while the file on
//! disk already carries the new one.
//!
//! The regression in 0.16.21 matched only the running image's pathname against
//! the delivered root. It therefore skipped this unit: the process mapped the
//! private path while the on-disk declaration named the root. Convergence must
//! reload the changed definition through the same observed launchd domain,
//! prove the replacement maps the delivered inode, and leave that replacement
//! alone on a repeated convergence.

#![cfg(target_os = "macos")]

use sha2::Digest;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HOST: &str = "probierz-native-readers-host";
const PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

mod cases;
mod fixture;

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
    label: String,
    domain: String,
    plist: PathBuf,
    root_binary: PathBuf,
    private_binary: PathBuf,
    archive: PathBuf,
    archive_sha256: String,
    port: u16,
    cleanup_finished: bool,
}

fn write_stado_archive(path: &Path, binary: &Path, member: &str) {
    let compressed = flate2::write::GzEncoder::new(
        fs::File::create(path).expect("create real Stado archive"),
        flate2::Compression::fast(),
    );
    let mut package = tar::Builder::new(compressed);
    package
        .append_path_with_name(binary, member)
        .expect("archive the built Stado executable");
    package
        .into_inner()
        .expect("finish native archive")
        .finish()
        .expect("finish native archive compression");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleanup_finished {
            if let Err(error) = self.cleanup() {
                eprintln!("native-reader fixture cleanup failure: {error}");
            }
        }
    }
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("hostname runs");
    assert!(
        output.status.success(),
        "hostname failed: {}",
        said(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn available_login_domain() -> String {
    let uid = unsafe { nix::libc::geteuid() };
    // Match the product's LaunchAgent placement in an active login session.
    for domain in [format!("gui/{uid}"), format!("user/{uid}")] {
        let output = Command::new("/bin/launchctl")
            .args(["print", &domain])
            .output()
            .expect("launchctl domain probe runs");
        if output.status.success() {
            return domain;
        }
    }
    panic!("launchd exposes neither the gui/{uid} nor user/{uid} login domain");
}

fn unused_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve a loopback port");
    listener.local_addr().expect("loopback address").port()
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn said(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct FileIdentity {
    device: u64,
    inode: u64,
    sha256: String,
}

fn file_identity(path: &Path) -> FileIdentity {
    let mut file = fs::File::open(path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
    let metadata = file
        .metadata()
        .unwrap_or_else(|error| panic!("cannot identify {}: {error}", path.display()));
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("cannot hash {}: {error}", path.display()));
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        sha256: hex::encode(hash.finalize()),
    }
}

fn assert_maps(
    fixture: &Fixture,
    pid: u32,
    expected_path: &Path,
    expected: &FileIdentity,
) -> serde_json::Value {
    let report = fixture.observed_image(pid);
    assert_eq!(
        report["process_device"], expected.device,
        "public label-print observed the wrong mapped device: {report}"
    );
    assert_eq!(
        report["process_inode"], expected.inode,
        "public label-print observed the wrong mapped inode: {report}"
    );
    assert_eq!(
        report["process_executable"],
        expected_path.to_string_lossy().into_owned(),
        "public label-print observed the wrong executable path: {report}"
    );
    assert_eq!(
        report["process_sha256"], expected.sha256,
        "public label-print did not hash the copied executable bytes: {report}"
    );
    report
}

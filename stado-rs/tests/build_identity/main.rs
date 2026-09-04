//! `stado --version` must say which tree the binary came from.
//!
//! The contract this defends: an operator asking a host what it is running
//! gets an answer, without dissecting a binary. On 2026-09-03 the version
//! `0.14.6` named four materially different trees of this crate — the one the
//! fleet was running (lacking the janitor workload-hold fix and the builder
//! claimability fix), two separate commits that each declared `0.14.6`, and a
//! local build with a fourth combination — and no release object existed to
//! tell them apart. Establishing what was actually deployed took `strings` and
//! `nm` against the installed binary. This test is why that is no longer the
//! way to find out.
//!
//! It drives the real binary rather than reading the constant, because the
//! constant being right is not the contract: the contract is that the shipped
//! executable prints it.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

/// The executable cargo just built for this test run.
const STADO: &str = env!("CARGO_BIN_EXE_stado");

/// The crate version the test binary was compiled against. The binary under
/// test is built from the same tree in the same cargo invocation.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `--version` as the shipped binary prints it.
fn version_line() -> String {
    let output = Command::new(STADO)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("{STADO} --version did not run: {error}"));
    assert!(
        output.status.success(),
        "{STADO} --version exited {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("--version is utf-8")
        .trim()
        .to_string()
}

#[test]
fn the_version_line_names_the_semantic_version() {
    let printed = version_line();
    assert!(
        printed.contains(VERSION),
        "{printed:?} does not name version {VERSION}"
    );
}

/// The defect itself. Before the fix this line was `stado 0.14.6` and stopped
/// there, which is exactly the state that made four trees indistinguishable.
#[test]
fn the_version_line_names_the_tree_it_was_built_from() {
    let printed = version_line();
    let revision = printed
        .split_once("(rev ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(revision, _)| revision.trim().to_string());
    let revision = revision.unwrap_or_else(|| {
        panic!(
            "{printed:?} carries no revision. The version alone does not identify \
             content: 0.14.6 named four different trees of this crate, and answering \
             'which build is this' meant reading symbols out of the binary."
        )
    });
    assert!(
        !revision.is_empty(),
        "{printed:?} names an empty revision, which answers nothing"
    );
    // Either a git revision, or the stated sentinel for a build context that
    // has no git metadata. A tarball build is legitimate and must still print
    // a usable line.
    if revision == "unknown" {
        return;
    }
    let core = revision.strip_suffix("-dirty").unwrap_or(&revision);
    assert!(
        core.len() >= 7 && core.len() <= 40,
        "{core:?} is not a git revision length"
    );
    assert!(
        core.chars().all(|character| character.is_ascii_hexdigit()),
        "{core:?} is not hexadecimal, so it is not a revision"
    );
}

/// One line, so it can be read out of a log or a `--version` capture without
/// anybody having to know how many lines to expect.
#[test]
fn the_version_line_is_one_line() {
    let printed = version_line();
    assert_eq!(
        printed.lines().count(),
        1,
        "--version printed {} lines: {printed:?}",
        printed.lines().count()
    );
}

/// A build in this repository, with git present, must name a real revision
/// rather than falling back to the sentinel. Skipped only where the fallback
/// is legitimate, which is a checkout with no git metadata at all.
#[test]
fn a_git_checkout_names_a_real_revision() {
    let git_available = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !git_available {
        return;
    }
    let printed = version_line();
    assert!(
        !printed.contains("(rev unknown)"),
        "{printed:?} fell back to the sentinel inside a git checkout, so the build \
         script failed to read a revision it could have read"
    );
}

/// The agent's release handoff reads the managed binary's `--version` line.
/// The revision suffix must not be mistaken for the semantic version: doing so
/// turns a stale marker into an endless supervised crash loop even though the
/// running and installed binaries are already identical.
#[test]
fn agent_repairs_a_stale_release_marker_when_managed_binary_is_current() {
    let operator_home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let scratch = operator_home.join(".stado/work");
    fs::create_dir_all(&scratch).expect("Stado scratch root exists");
    let fixture = tempfile::Builder::new()
        .prefix("agent-release-marker-")
        .tempdir_in(&scratch)
        .expect("isolated agent fixture");
    let home = fixture.path().join("home");
    let bin = home.join(".stado/bin");
    fs::create_dir_all(&bin).expect("managed binary directory exists");
    let managed = bin.join("stado");
    fs::copy(STADO, &managed).expect("the real Stado binary is installed");
    fs::set_permissions(&managed, fs::Permissions::from_mode(0o755))
        .expect("the managed Stado binary is executable");
    let marker = bin.join("stado.release-version");
    fs::write(&marker, "0.0.0\n").expect("stale release marker is seeded");

    let hostname_output = Command::new("hostname")
        .arg("-f")
        .output()
        .expect("hostname command starts");
    assert!(
        hostname_output.status.success(),
        "hostname -f failed: {}",
        String::from_utf8_lossy(&hostname_output.stderr)
    );
    let hostname = String::from_utf8(hostname_output.stdout)
        .expect("hostname is utf-8")
        .trim()
        .to_ascii_lowercase();
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("agent fixture has no platform mapping for {os}-{arch}"),
    };
    let storage = fixture.path().join("storage");
    fs::create_dir_all(&storage).expect("isolated queue store exists");
    fs::write(
        storage.join("registry.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 2,
            "targets": [{
                "name": "build-identity-runner",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform,
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 1,
                    "target_free_gb": 2,
                    "max_bytes_per_pass": 1048576,
                    "max_items_per_pass": 1,
                    "max_scan_items": 1,
                    "cleaners": {}
                },
                "slots": 1
            }]
        }))
        .expect("registry encodes"),
    )
    .expect("registry is written");

    let stdout_path = fixture.path().join("agent.out");
    let stderr_path = fixture.path().join("agent.err");
    let stdout = File::create(&stdout_path).expect("agent stdout opens");
    let stderr = File::create(&stderr_path).expect("agent stderr opens");
    let mut agent = Command::new(&managed)
        .args(["agent", "--auto"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("STADO_CONFIG", home.join("nonexistent-config.json"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", &storage)
        .env("WC_STADO_STORAGE_NAMESPACE", "build-identity")
        .env("WC_VAST_AUTO_LIST", "false")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("managed Stado agent starts");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut observed = String::new();
    while Instant::now() < deadline {
        observed = fs::read_to_string(&marker).unwrap_or_default();
        if observed.trim() == VERSION {
            break;
        }
        if agent
            .try_wait()
            .expect("agent status is readable")
            .is_some()
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = agent.kill();
    let status = agent.wait().expect("agent can be reaped");
    let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert_eq!(
        observed.trim(),
        VERSION,
        "the current managed binary must repair the stale marker instead of entering a handoff \
         loop; agent status: {status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

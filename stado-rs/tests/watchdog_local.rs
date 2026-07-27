//! End-to-end `stado-watchdog` tests: the real binary with stubbed
//! diagnostic commands on PATH (TempDir bin/ with canned
//! systemctl/journalctl/ps/nvidia-smi/df/free/gcloud shell scripts) and
//! the local storage backend.

use std::path::Path;
use std::process::{Command, Output};

fn write_stub(bin_dir: &Path, name: &str, body: &str) {
    let path = bin_dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Stub every command the watchdog collects. `systemctl` exits 3 to prove
/// nonzero-rc isolation; canned stdout makes each entry identifiable.
fn stub_bin(dir: &Path) -> std::path::PathBuf {
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_stub(&bin_dir, "systemctl", "printf 'stub-systemctl\\n'\nexit 3");
    write_stub(&bin_dir, "journalctl", "printf 'stub-journalctl\\n'");
    write_stub(&bin_dir, "ps", "printf 'stub-ps\\n'");
    write_stub(&bin_dir, "nvidia-smi", "printf 'stub-nvidia-smi\\n'");
    write_stub(&bin_dir, "df", "printf 'stub-df\\n'");
    write_stub(&bin_dir, "free", "printf 'stub-free\\n'");
    write_stub(&bin_dir, "gcloud", "printf 'stub-gcloud %s\\n' \"$4\"");
    bin_dir
}

fn watchdog(bin_dir: &Path, storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado-watchdog"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("HOSTNAME", "testbox01")
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", bin_dir.display()),
        );
    cmd.output().expect("stado-watchdog binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn once_collects_and_uploads_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let bin_dir = stub_bin(dir.path());

    let out = watchdog(&bin_dir, &storage, &["--once", "--bucket", "test-bucket"]);
    assert!(out.status.success(), "exit: {}", stderr(&out));
    assert_eq!(out.status.code(), Some(0));
    let printed = stdout(&out);
    assert!(
        printed.trim_end().ends_with("uploaded box_diagnostics/testbox01.json"),
        "{printed}"
    );

    // Both blob shapes exist and are identical.
    let flat = storage.join("box_diagnostics/testbox01.json");
    let nested = storage.join("box_diagnostics/testbox01/latest.json");
    assert!(flat.exists(), "flat diagnostics blob");
    assert!(nested.exists(), "latest.json diagnostics blob");
    let text = std::fs::read_to_string(&flat).unwrap();
    assert_eq!(text, std::fs::read_to_string(&nested).unwrap());
    // json.dumps(indent=2, sort_keys=True).
    assert!(text.find("\"bucket\"").unwrap() < text.find("\"commands\"").unwrap(), "{text}");

    let payload: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["schema"], "wisent-box-diagnostics-v1");
    assert_eq!(payload["host"], "testbox01");
    assert_eq!(payload["bucket"], "test-bucket");
    assert!(payload["pid"].as_u64().unwrap() > 0);
    assert_eq!(payload["disk"].as_array().unwrap().len(), 3);

    let commands = payload["commands"].as_object().unwrap();
    assert_eq!(commands.len(), 9);
    // Nonzero rc kept, not treated as an error.
    assert_eq!(commands["systemctl-agent"]["rc"], 3);
    assert_eq!(commands["systemctl-agent"]["stdout_tail"], "stub-systemctl\n");
    assert_eq!(commands["nvidia-smi"]["stdout_tail"], "stub-nvidia-smi\n");
    // Exact argv including the bucket interpolation.
    assert_eq!(
        commands["capacity-list"]["cmd"],
        serde_json::json!(["gcloud", "--quiet", "storage", "ls", "gs://test-bucket/capacity/"])
    );
    assert_eq!(commands["capacity-list"]["stdout_tail"], "stub-gcloud gs://test-bucket/capacity/\n");
}

#[test]
fn argparse_error_format_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let bin_dir = stub_bin(dir.path());
    let usage = "usage: stado-watchdog [-h] [--bucket BUCKET] [--interval-s INTERVAL_S]\n                      [--once]";

    let out = watchdog(&bin_dir, &storage, &["--bogus"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stderr(&out),
        format!("{usage}\nstado-watchdog: error: unrecognized arguments: --bogus\n")
    );

    let out = watchdog(&bin_dir, &storage, &["--interval-s", "abc"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        format!("{usage}\nstado-watchdog: error: argument --interval-s: invalid int value: 'abc'\n")
    );

    let out = watchdog(&bin_dir, &storage, &["--bucket"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        format!("{usage}\nstado-watchdog: error: argument --bucket: expected one argument\n")
    );

    let out = watchdog(&bin_dir, &storage, &["--help"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let help = stdout(&out);
    assert!(help.starts_with(&format!("{usage}\n\nUpload workstation diagnostics to GCS.\n")), "{help}");
    assert!(help.contains("options:\n  -h, --help            show this help message and exit\n"));
}

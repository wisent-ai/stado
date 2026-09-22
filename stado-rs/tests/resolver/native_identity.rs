//! Read-only native identity qualification; no unit is installed or restarted.
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{fixture::Policy, Host, TARGET};

#[test]
#[ignore = "requires STADO_TEST_NATIVE_LABEL naming a stable, directly launched system service"]
fn native_identity_matches_the_resident_process_and_its_actual_image() {
    let label = std::env::var("STADO_TEST_NATIVE_LABEL").expect("declare the native service label");
    let qualified = format!("system/{label}");
    let native = Command::new("/bin/launchctl")
        .args(["print", &qualified])
        .output()
        .expect("read the actual system launchd job");
    eprintln!(
        "launchctl print {qualified}: exit={:?}\n{}\n{}",
        native.status.code(),
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr),
    );
    assert!(
        native.status.success(),
        "native service prerequisite failed"
    );
    let text = String::from_utf8(native.stdout).expect("native job description");
    let field = |name: &str| {
        let prefix = format!("\t{name} = ");
        text.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(str::trim)
            .unwrap_or_else(|| panic!("native job has no {name}"))
    };
    let pid = field("pid");
    let image = Path::new(field("program"));
    let metadata = std::fs::metadata(image).expect("read actual native executable metadata");
    let (_, digest) = stado::release_control::sha256_file(image).expect("hash actual native image");
    let started = Command::new("/bin/ps")
        .args(["-p", pid, "-o", "lstart="])
        .output()
        .expect("read kernel process start time");
    assert!(started.status.success(), "native process disappeared");
    let started = String::from_utf8(started.stdout)
        .expect("process start text")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let host = Host::new(&Policy::patient(1).document());
    let answer = host.stado(&[
        "service",
        "label-print",
        "--host",
        TARGET,
        "--domain",
        "system",
        &label,
        "--json",
    ]);
    eprintln!(
        "source={} label-print {label}: exit={:?}\n{}\n{}",
        env!("STADO_SOURCE_REVISION"),
        answer.status.code(),
        String::from_utf8_lossy(&answer.stdout),
        String::from_utf8_lossy(&answer.stderr),
    );
    assert!(answer.status.success(), "Stado native inspection failed");
    let report: Value = serde_json::from_slice(&answer.stdout).expect("native identity report");
    assert_eq!(report["loaded"], true, "{report}");
    assert_eq!(report["pid"], pid, "{report}");
    assert_eq!(report["process_device"], metadata.dev(), "{report}");
    assert_eq!(report["process_inode"], metadata.ino(), "{report}");
    assert_eq!(report["process_sha256"], digest, "{report}");
    assert_eq!(report["process_started_at"], started, "{report}");
    assert!(report["process_identity_unavailable"].is_null(), "{report}");
    let observed = report["process_executable"]
        .as_str()
        .expect("observed image");
    assert_eq!(
        std::fs::canonicalize(observed).expect("resolve observed image"),
        std::fs::canonicalize(image).expect("resolve independently read image"),
    );
}

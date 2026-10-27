//! `space report` walks the home once for its inventory: every root below the
//! home is answered from that one `du`, and a root's rows still reach as deep
//! as that root declares. The walks are read from the process table while the
//! real report runs, so a report that walked `.local/share` again after the
//! home is seen doing it.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::native::Native;

/// Every `du` the process table shows at this moment, as its arguments.
fn walks(native: &Native) -> Vec<String> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "args="])
        .output()
        .expect("ps runs");
    native.record("/bin/ps", &["-axo", "args="], &output);
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("/usr/bin/du "))
        .map(str::to_string)
        .collect()
}

/// How long one look at the process table takes.
fn sample_time(native: &Native) -> Duration {
    let started = Instant::now();
    walks(native);
    started.elapsed()
}

/// How long one `du` of `tree` takes.
fn walk_time(tree: &Path) -> Duration {
    let started = Instant::now();
    let status = Command::new("/usr/bin/du")
        .args(["-xk", tree.to_str().unwrap()])
        .output()
        .expect("du runs")
        .status;
    assert!(status.success(), "du of {} failed", tree.display());
    started.elapsed()
}

/// Grow `tree` by one child in every directory it holds until one walk of it
/// lasts longer than two looks at the process table, so no walk of it can
/// start and end between two looks.
fn outlast_sampling(native: &Native, tree: &Path) {
    let mut held: Vec<PathBuf> = vec![tree.to_path_buf()];
    fs::create_dir_all(tree).unwrap();
    while walk_time(tree) <= sample_time(native) + sample_time(native) {
        // A name no earlier round used: the count of directories so far plus
        // this parent's place, so a parent grown before gets a new child.
        let grown: Vec<PathBuf> = held
            .iter()
            .enumerate()
            .map(|(index, parent)| parent.join((held.len() + index).to_string()))
            .collect();
        for directory in &grown {
            fs::create_dir(directory).unwrap();
        }
        held.extend(grown);
    }
}

#[test]
fn a_space_report_walks_the_home_once() {
    let native = Native::new("space-report-one-walk");
    let share = native.home.join(".local/share");
    // `.local/share` is reported four levels deep, so `deep` is a row of it
    // the home's own depth-two rows never reach. `probe` is three levels
    // below the home and under no deeper root: the one walk of the home lists
    // it, and no root may report it.
    let deep = share.join("product/kept/level/deep");
    let probe = native.home.join("work/one/probe");
    let block = usize::try_from(fs::metadata(&native.home).unwrap().blksize()).unwrap();
    for directory in [&deep, &probe] {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join("data"), vec![b'x'; block]).unwrap();
    }
    // The tree that makes a walk of `.local/share` last sits below its
    // reported depth, so its rows do not crowd out `deep`.
    outlast_sampling(&native, &share.join("slow/to/walk/below/bulk"));

    let args = ["space", "report", "example-cleanup-host", "--json"];
    let mut child = native
        .command(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the real space report");
    let mut seen = BTreeSet::new();
    while child.try_wait().expect("read the report's state").is_none() {
        seen.extend(walks(&native));
    }
    let output = child.wait_with_output().expect("read the report's output");
    native.record(env!("CARGO_BIN_EXE_stado"), &args, &output);
    assert!(
        output.status.success(),
        "space report exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    native.observe("space report", report.clone());
    native.observe(
        "du walks seen",
        serde_json::json!(seen.iter().collect::<Vec<_>>()),
    );
    assert!(
        report
            .get("inventory_incomplete")
            .is_none_or(|it| it.is_null()),
        "the inventory did not run: {report}"
    );

    let home = native.home.to_string_lossy().to_string();
    let inside = format!("{home}/");
    let of_home: Vec<&String> = seen.iter().filter(|walk| walk.contains(&home)).collect();
    assert!(
        of_home
            .iter()
            .any(|walk| walk.ends_with(&format!(" {home}"))),
        "no walk of the home was seen: {seen:?}"
    );
    assert!(
        of_home.iter().all(|walk| !walk.contains(&inside)),
        "a root below the home was walked again: {of_home:?}"
    );

    let inventory = report["inventory"].as_array().expect("inventory rows");
    let reported = |path: &Path| {
        let path = path.to_string_lossy();
        inventory.iter().any(|row| row["path"] == path.as_ref())
    };
    assert!(
        reported(&share),
        "{} is not reported: {report}",
        share.display()
    );
    assert!(
        reported(&deep),
        "{} is not reported from the home walk: {report}",
        deep.display()
    );
    assert!(
        !reported(&probe),
        "{} is reported though it is deeper than the home's rows and under no deeper root: {report}",
        probe.display()
    );
}

#[test]
fn an_inventory_skipped_under_load_reports_unknown_totals_not_zero() {
    let native = Native::new("space-report-incomplete");
    let args = ["space", "report", "example-cleanup-host", "--json"];
    let ceiling = f64::MIN_POSITIVE.to_string();
    let output = native.command(&args)
        .env("WC_HOST_SCAN_LOAD_CEILING", &ceiling)
        .output()
        .expect("the real report runs with the explicit scan ceiling");
    native.record(env!("CARGO_BIN_EXE_stado"), &args, &output);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the real report is JSON");
    native.observe("incomplete space report", report.clone());
    let reason = report["inventory_incomplete"].as_str().expect("the real inventory was refused under load");
    assert!(reason.contains("inventory walk skipped") && reason.contains("ceiling"), "{reason}");
    assert!(report["usage"]["available_kb"].as_str().is_some(), "the actual df observation was lost: {report}");
    let coverage = &report["coverage"];
    for field in ["covered_bytes", "uncovered_bytes", "cleaner_bytes", "unswept_bytes"] {
        assert_eq!(coverage.get(field), Some(&serde_json::Value::Null),
            "an incomplete inventory invented a whole-inventory total for {field}: {report}");
    }
    assert_eq!(coverage["inventory_incomplete"], report["inventory_incomplete"],
        "the coverage must retain the actual failed inventory operation");
}

/// The installed memory as this machine's own tools state it, in bytes:
/// `sysctl hw.memsize` on macOS, `free -b`'s total on Linux.
fn kernel_total_bytes(native: &Native) -> i64 {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("/usr/sbin/sysctl", &["-n", "hw.memsize"])
    } else {
        ("free", &["-b"])
    };
    let output = Command::new(program).args(args).output().expect("the memory tool runs");
    native.record(program, args, &output);
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let figure = if cfg!(target_os = "macos") {
        text.trim().to_string()
    } else {
        text.lines()
            .find_map(|line| line.strip_prefix("Mem:"))
            .and_then(|rest| rest.split_whitespace().next())
            .expect("free states a Mem: total")
            .to_string()
    };
    figure.parse().expect("the installed memory is a byte count")
}

#[test]
fn a_space_report_reads_memory_as_the_kernel_states_it() {
    let native = Native::new("space-report-memory");
    let args = ["space", "report", "example-cleanup-host", "--json"];
    let output = native.command(&args).output().expect("run the real space report");
    native.record(env!("CARGO_BIN_EXE_stado"), &args, &output);
    assert!(
        output.status.success(),
        "space report exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let memory = &report["memory"];
    native.observe("memory", memory.clone());
    // A host whose kernel answers every figure leaves nothing unread.
    assert!(memory["unread"].is_null(), "memory was not fully read: {memory}");
    let total = memory["total_bytes"].as_i64().expect("total_bytes is a count");
    assert_eq!(total, kernel_total_bytes(&native), "total_bytes is not the kernel's: {memory}");
    let available = memory["available_bytes"].as_i64().expect("available_bytes is a count");
    assert!(
        available.is_positive() && available <= total,
        "available_bytes {available} is not a share of the installed {total}: {memory}"
    );
    if cfg!(target_os = "macos") {
        // The page size comes from the vm_stat banner, as the host's own pass reads it.
        let args = ["-n", "hw.pagesize"];
        let page = Command::new("/usr/sbin/sysctl").args(args).output().expect("sysctl runs");
        native.record("/usr/sbin/sysctl", &args, &page);
        let page: i64 = String::from_utf8_lossy(&page.stdout).trim().parse().expect("hw.pagesize is an integer");
        assert_eq!(memory["page_size_bytes"].as_i64(), Some(page), "page size is not the kernel's: {memory}");
    }
}

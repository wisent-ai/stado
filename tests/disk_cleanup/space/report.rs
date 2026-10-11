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
        let grown: Vec<PathBuf> = held
            .iter()
            .enumerate()
            .map(|(index, parent)| parent.join(index.to_string()))
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

//! At the disk-full rule the janitor takes each of Xcode's per-project build
//! directories under `~/Library/Developer/Xcode/DerivedData` whole, keeps one
//! a running process has a file in open, never touches the shared module
//! cache, archives or device support, and records what it took. Run on a
//! real APFS volume under the repository's build directory.

use std::fs;
use std::process::{Command, Stdio};

use crate::native::Native;

/// Xcode's 28-letter workspace hash, as it appends one to a project's name.
const HASH: &str = "abcdefghijklmnopqrstuvwxyzab";

#[test]
fn an_unheld_xcode_project_build_goes_at_the_rule_and_a_held_one_stays() {
    let native = Native::new("derived-data");
    let derived = native.home.join("Library/Developer/Xcode/DerivedData");
    let idle = derived.join(format!("StadoDesktop-{HASH}"));
    let building = derived.join(format!("BramaDesktop-{}", HASH.replace('a', "q")));
    let named = derived.join(format!("OkoDesktop-{}", HASH.replace('a', "r")));
    let module_cache = derived.join("ModuleCache.noindex");
    let archives = native
        .home
        .join("Library/Developer/Xcode/Archives/app.xcarchive");
    let device_support = native
        .home
        .join("Library/Developer/Xcode/iOS DeviceSupport/18.0");
    for tree in [
        &idle,
        &building,
        &named,
        &module_cache,
        &archives,
        &device_support,
    ] {
        fs::create_dir_all(tree.join("Build/Intermediates.noindex")).unwrap();
        fs::write(
            tree.join("Build/Intermediates.noindex/object.o"),
            vec![b'x'; 64 * 1024],
        )
        .unwrap();
    }
    // A build writing into its project directory holds a file open there,
    // as xcodebuild does.
    let held_file = building.join("Build/Intermediates.noindex/object.o");
    let mut build = Command::new("/usr/bin/tail")
        .args(["-f", held_file.to_str().unwrap()])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    // A tool that names a project directory in its arguments without
    // opening a file there yet (`xcodebuild -derivedDataPath <dir>`): a
    // shell that stays itself (two commands, so it does not exec the first)
    // with the path as its `$0`.
    let mut naming = Command::new("/bin/sh")
        .args(["-c", "sleep 600; true", named.to_str().unwrap()])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    let ranked = native.json(&["space", "candidates", "--min-bytes", "1", "--json"]);
    native.observe("candidates", ranked.clone());
    let rows = ranked["candidates"].as_array().unwrap();
    for project in [&idle, &building] {
        let row = rows
            .iter()
            .find(|row| row["path"].as_str() == project.to_str())
            .unwrap_or_else(|| panic!("{} is not ranked: {ranked}", project.display()));
        assert_eq!(row["class"], "agent_output", "{row}");
        assert_eq!(row["action"], "automatic", "{row}");
        assert_eq!(row["cleaner"], "xcode_derived_data", "{row}");
    }

    let user_data = native.fill_with_user_data();
    let report = native.cleanup();
    let cleaner = &report["cleaners"]["xcode_derived_data"];
    build.kill().unwrap();
    build.wait().unwrap();
    naming.kill().unwrap();
    naming.wait().unwrap();
    assert_eq!(cleaner["deleted_items"], 1, "{report}");
    assert_eq!(cleaner["skipped"]["held_by_process"], 2, "{report}");
    assert!(
        named.join("Build/Intermediates.noindex/object.o").is_file(),
        "a project directory a process names was taken: {report}"
    );
    assert!(!idle.exists(), "an unheld project build was kept: {report}");
    assert!(
        held_file.is_file(),
        "a project build a process holds was taken: {report}"
    );
    for kept in [&module_cache, &archives, &device_support] {
        assert!(
            kept.join("Build/Intermediates.noindex/object.o").is_file(),
            "{} was taken: {report}",
            kept.display()
        );
    }
    assert!(user_data.is_file(), "the user's data was deleted");

    let after = native.json(&["space", "candidates", "--json"]);
    assert!(
        after["removed"].as_array().unwrap().iter().any(|line| {
            line["cleaner"] == "xcode_derived_data"
                && line["path"].as_str() == idle.to_str()
                && line["bytes"].as_i64().is_some_and(|bytes| bytes > 0)
        }),
        "the removal is not recorded: {after}"
    );
}

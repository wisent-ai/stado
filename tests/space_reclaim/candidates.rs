//! `stado space candidates` ranks what holds the disk — agent output first,
//! then caches, then the user's own large items as proposals — and the
//! janitor's pass at the disk-full rule takes only the agent output: Cargo's
//! per-workspace build directories (tagged or not) and tagged build trees,
//! never a cache it does not own or a user's file, and records each tree it
//! took. Run on a real APFS volume under the repository's build directory.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::Path;

use serde_json::Value;

use crate::native::Native;

/// One file of `bytes` at `path`, its directories made first.
fn file(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

/// A Cargo build directory as `build.build-dir` lays one out under Cargo's
/// home, untagged, with both build locks a Cargo release may hold in its
/// profile directory.
fn cargo_build_dir(path: &Path) {
    file(&path.join("debug/deps/libexample.rlib"), 64 * 1024);
    fs::write(path.join("debug/.cargo-lock"), b"").unwrap();
    fs::write(path.join("debug/.cargo-build-lock"), b"").unwrap();
}

/// The row naming `path`, if the ranking lists it.
fn row<'a>(report: &'a Value, path: &Path) -> Option<&'a Value> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"].as_str().map(Path::new) == Some(path.as_path()))
}

fn candidates(native: &Native) -> Value {
    let report = native.json(&["space", "candidates", "--min-bytes", "1", "--json"]);
    native.observe("space candidates", report.clone());
    report
}

#[test]
fn candidates_rank_agent_output_first_and_the_pass_takes_only_agent_output() {
    let native = Native::new("candidates");
    let home = &native.home;
    let idle_build = home.join(".cargo/build/ab/0123456789abcdef");
    let held_build = home.join(".cargo/build/cd/fedcba9876543210");
    cargo_build_dir(&idle_build);
    cargo_build_dir(&held_build);
    let tagged = home.join("checkout/target");
    native.cache(&tagged);
    let registry = home.join(".cargo/registry/cache/example.crate");
    file(&registry, 32 * 1024);
    let npm = home.join(".npm/_cacache/content-v2/blob");
    file(&npm, 32 * 1024);
    let game = home.join("Games/downloaded-game.pak");
    file(&game, 256 * 1024);

    let before = candidates(&native);
    assert_eq!(before["rule"]["triggered"], false, "{before}");
    let rows = before["candidates"].as_array().unwrap();
    let order: Vec<&str> = rows
        .iter()
        .map(|row| row["class"].as_str().unwrap())
        .collect();
    let rank = |class: &str| match class {
        "agent_output" => 0,
        "cache" => 1,
        "user_data" => 2,
        other => panic!("unknown class {other}: {before}"),
    };
    assert!(
        order.windows(2).all(|pair| rank(pair[0]) <= rank(pair[1])),
        "agent output is not ranked before caches and caches before user data: {order:?}"
    );
    for (path, class, action) in [
        (&idle_build, "agent_output", "automatic"),
        (&held_build, "agent_output", "automatic"),
        (&tagged, "agent_output", "automatic"),
        (&home.join(".cargo/registry"), "cache", "proposal"),
        // A package manager's download cache is the janitor's second tier.
        (&home.join(".npm/_cacache"), "cache", "automatic"),
        (&game, "user_data", "proposal"),
    ] {
        let found = row(&before, path)
            .unwrap_or_else(|| panic!("{} is not ranked: {before}", path.display()));
        assert_eq!(found["class"], class, "{found}");
        assert_eq!(found["action"], action, "{found}");
        assert!(found["bytes"].as_i64().unwrap() > 0, "{found}");
        assert!(
            found["producer"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "{found}"
        );
        assert!(found["safety"].is_string(), "{found}");
    }
    assert_eq!(row(&before, &game).unwrap()["safety"], "user_owned");
    assert!(
        idle_build.is_dir() && tagged.is_dir(),
        "a read deleted something"
    );

    // A build running in one Cargo build directory holds its lock, as
    // `cargo build` does with a build-dir under Cargo's home: the
    // `.cargo-build-lock`, never `.cargo-lock`.
    let lock = File::options()
        .read(true)
        .write(true)
        .open(held_build.join("debug/.cargo-build-lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns, released when `lock` drops.
    let locked =
        unsafe { nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    assert!(
        !locked.is_negative(),
        "the test could not hold the build lock"
    );

    let user_data = native.fill_with_user_data();
    let full = candidates(&native);
    assert_eq!(full["rule"]["triggered"], true, "{full}");
    assert!(full["need_bytes"].as_i64().unwrap() > 0, "{full}");

    let report = native.cleanup();
    let cargo = &report["cleaners"]["cargo_build_dirs"];
    assert_eq!(cargo["deleted_items"], 1, "{report}");
    assert_eq!(cargo["skipped"]["held_by_build"], 1, "{report}");
    assert!(
        !idle_build.exists(),
        "an unheld Cargo build directory was kept: {report}"
    );
    assert!(
        held_build.join("debug/deps/libexample.rlib").is_file(),
        "a Cargo build directory a running build holds was taken: {report}"
    );
    assert!(!tagged.exists(), "a tagged build tree was kept: {report}");
    // Cargo's registry and the owner's files are never the janitor's; the
    // npm cache is its second tier, taken only if agent output was not
    // enough (tests/space_reclaim/package_caches.rs).
    for kept in [&registry, &game, &user_data] {
        assert!(kept.is_file(), "{} was deleted: {report}", kept.display());
    }
    drop(lock);

    let after = candidates(&native);
    let removed = after["removed"].as_array().unwrap();
    for (path, cleaner) in [(&idle_build, "cargo_build_dirs"), (&tagged, "build_caches")] {
        assert!(
            removed.iter().any(|line| {
                line["cleaner"] == cleaner
                    && line["path"].as_str().is_some_and(|recorded| {
                        recorded.ends_with(path.strip_prefix(home).unwrap().to_str().unwrap())
                    })
                    && line["bytes"].as_i64().is_some_and(|bytes| bytes > 0)
            }),
            "the removal of {} is not recorded: {after}",
            path.display()
        );
    }
    assert!(
        row(&after, &game).is_some(),
        "the user's game left the proposals: {after}"
    );
}

#[test]
fn a_damaged_removal_record_is_refused_naming_its_line() {
    let native = Native::new("candidates-ledger");
    let state = native.state_dir();
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("reclaimed.jsonl"),
        b"{\"cleaner\":\"build_caches\",\"path\":\"/x\",\"bytes\":1}\nnot a record\n",
    )
    .unwrap();
    let output = native.run(&["space", "candidates"]);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    native.observe(
        "damaged record",
        serde_json::json!({"status": output.status.code(), "stderr": stderr}),
    );
    assert!(
        !output.status.success(),
        "a damaged record was read: {stderr}"
    );
    assert!(
        stderr.contains("line 2 of the janitor's removal record")
            && stderr.contains("move it aside"),
        "the refusal does not name the line and the fix: {stderr}"
    );
}

#[test]
fn a_home_that_is_not_a_directory_is_refused_naming_it() {
    let native = Native::new("candidates-home");
    let not_a_home = native.home.join("not-a-home");
    fs::write(&not_a_home, b"a file, not a directory\n").unwrap();
    let args = ["space", "candidates"];
    let output = native
        .command(&args)
        .env("HOME", &not_a_home)
        .output()
        .unwrap();
    native.record(env!("CARGO_BIN_EXE_stado"), &args, &output);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "a file was surveyed as a home: {stderr}"
    );
    assert!(
        stderr.contains("not-a-home")
            && stderr.contains("it is not a directory")
            && stderr.contains("HOME must name"),
        "the refusal does not name the home, the cause and the fix: {stderr}"
    );
}

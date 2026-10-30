//! The package managers' download caches are the janitor's second tier: at
//! the disk-full rule a pass takes them only when the agent output it took
//! left the volume at the rule, or without the room under it a build of this
//! host takes, never one a running process holds, and records each. When
//! agent output alone leaves that room, every cache stays. Run on a real
//! APFS volume under the repository's build directory.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::native::Native;

/// `bytes` of data at `path`, its directories made first, flushed.
fn file(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = File::create(path).unwrap();
    out.write_all(&vec![b'x'; bytes]).unwrap();
    out.sync_all().unwrap();
}

/// What the volume may still take before the rule: free bytes less the
/// rule's reserve, as `stado host gates` reports them.
fn headroom(native: &Native) -> i64 {
    let disk = native.disk();
    let free = disk["free_bytes"].as_i64().expect("free bytes");
    let percent = disk["full_percent"].as_i64().expect("full percent");
    let total = i64::try_from(fs2::total_space(&native.home).unwrap()).unwrap();
    free - total / 100 * (100 - percent)
}

#[test]
fn caches_go_only_when_agent_output_is_not_enough_and_never_while_held() {
    let native = Native::new("package-caches");
    let npm = native.home.join(".npm/_cacache/content-v2/blob");
    let bun = native.home.join(".bun/install/cache/package.tgz");
    let held = native.home.join(".cache/uv/wheels/held.whl");
    // The held cache is the largest, so the pass, which takes the largest
    // first, meets it before the volume is under the rule again.
    file(&held, 8 * 1024 * 1024);
    for cache in [&npm, &bun] {
        file(cache, 4 * 1024 * 1024);
    }
    // A running tool holds the uv cache open, as `uv sync` does.
    let mut holder = Command::new("/usr/bin/tail")
        .args(["-f", held.to_str().unwrap()])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    holder.kill().unwrap();
    holder.wait().unwrap();
    let cleaner = &report["cleaners"]["package_caches"];
    assert_eq!(cleaner["scanned_items"], 3, "{report}");
    assert!(
        cleaner["deleted_items"]
            .as_i64()
            .is_some_and(|items| items >= 1),
        "no cache was taken while agent output could not relieve the volume: {report}"
    );
    assert!(
        held.is_file(),
        "a cache a running process holds was taken: {report}"
    );
    assert_eq!(cleaner["skipped"]["held_by_process"], 1, "{report}");
    assert!(user_data.is_file(), "the user's data was deleted");
    let taken: Vec<PathBuf> = [&npm, &bun]
        .iter()
        .filter(|cache| !cache.exists())
        .map(|cache| (*cache).clone())
        .collect();
    assert!(!taken.is_empty(), "{report}");
    let record = native.json(&["space", "candidates", "--json"]);
    for cache in &taken {
        let root = if cache.starts_with(native.home.join(".npm")) {
            native.home.join(".npm/_cacache")
        } else {
            native.home.join(".bun/install/cache")
        };
        assert!(
            record["removed"].as_array().unwrap().iter().any(|line| {
                line["cleaner"] == "package_caches" && line["path"].as_str() == root.to_str()
            }),
            "the removal of {} is not recorded: {record}",
            root.display()
        );
    }
}

#[test]
fn every_cache_stays_when_agent_output_brings_the_volume_under_the_rule() {
    let native = Native::new("package-caches-enough");
    let npm = native.home.join(".npm/_cacache/content-v2/blob");
    file(&npm, 1024 * 1024);
    let user_data = native.fill_with_user_data();
    // Agent output larger than how far past the rule the volume is: a
    // tagged build tree the pass takes first.
    let past = -headroom(&native);
    let tree = native.home.join("checkout/target");
    native.cache(&tree);
    let needed = usize::try_from(past.max(0)).unwrap() + 8 * 1024 * 1024;
    // Room for the tree: the filler gives back what the tree takes.
    let filler = File::options().write(true).open(&user_data).unwrap();
    let length = filler.metadata().unwrap().len();
    filler
        .set_len(length - u64::try_from(needed).unwrap())
        .unwrap();
    filler.sync_all().unwrap();
    file(&tree.join("payload.bin"), needed);
    assert_eq!(native.disk()["full"], true, "the volume is not at the rule");

    let report = native.cleanup();
    assert!(!tree.exists(), "the tagged build tree was kept: {report}");
    assert!(
        npm.is_file(),
        "a cache went though agent output was enough: {report}"
    );
    let cleaner = &report["cleaners"]["package_caches"];
    assert_eq!(cleaner["deleted_items"], 0, "{report}");
    assert_eq!(cleaner["skipped"]["agent_output_was_enough"], 1, "{report}");
    assert!(user_data.is_file(), "the user's data was deleted");
}

/// One line of the janitor's removal record: a Cargo build directory of
/// `bytes` a pass took `days_ago` days ago, as the janitor appends it.
fn recorded_build(native: &Native, name: &str, bytes: i64, days_ago: i64) -> String {
    let at = chrono::Utc::now() - chrono::Duration::days(days_ago);
    let path = native.home.join(".cargo/build/ab").join(name);
    serde_json::json!({
        "removed_at": at.to_rfc3339(),
        "writer": "cli",
        "cleaner": "cargo_build_dirs",
        "class": "agent_output",
        "path": path,
        "bytes": bytes,
        "freed_bytes": bytes,
    })
    .to_string()
}

#[test]
fn under_the_rule_a_pass_still_makes_room_for_the_largest_recent_build() {
    let native = Native::new("package-caches-margin");
    const MIB: i64 = 1024 * 1024;
    let npm = native.home.join(".npm/_cacache/content-v2/blob");
    let bun = native.home.join(".bun/install/cache/package.tgz");
    let uv = native.home.join(".cache/uv/wheels/small.whl");
    file(&npm, 16 * 1024 * 1024);
    file(&bun, 12 * 1024 * 1024);
    file(&uv, 2 * 1024 * 1024);
    // An idle agent's Cargo build directory: agent output the rule alone
    // gives leave to take, so a pass under the rule keeps it, with its
    // incremental state, and only measures it.
    let idle_build = native
        .home
        .join(".cargo/build/cd/0011223344556677/debug/deps/libidle.rlib");
    file(&idle_build, 1024 * 1024);
    // A build of this host took 24 MiB last week; one a month ago took more
    // than the volume and no longer counts.
    let state = native.state_dir();
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("reclaimed.jsonl"),
        format!(
            "{}\n{}\n",
            recorded_build(&native, "0123456789abcdef", 24 * MIB, 2),
            recorded_build(&native, "fedcba9876543210", 1024 * MIB, 30)
        ),
    )
    .unwrap();
    // Under the rule by 4 MiB: no pass ever ran here before the margin.
    let user_data = native.fill_with_user_data();
    let past = -headroom(&native);
    let filler = File::options().write(true).open(&user_data).unwrap();
    let length = filler.metadata().unwrap().len();
    filler
        .set_len(length - u64::try_from(past.max(0) + 4 * MIB).unwrap())
        .unwrap();
    filler.sync_all().unwrap();
    assert_eq!(native.disk()["full"], false, "the volume is at the rule");
    assert!(
        headroom(&native) < 24 * MIB,
        "the volume has the room already"
    );

    let report = native.cleanup();
    assert_ne!(report["outcome"], "healthy_noop", "{report}");
    assert_eq!(report["margin"]["bytes"], 24 * MIB, "{report}");
    assert_eq!(report["margin"]["source"], "removal_record", "{report}");
    let cleaner = &report["cleaners"]["package_caches"];
    assert!(!npm.exists(), "the largest cache was kept: {report}");
    assert!(
        headroom(&native) >= 24 * MIB,
        "the pass ended without room for the recent build: {report}"
    );
    assert!(
        uv.is_file(),
        "a cache went after the volume had the room: {report}"
    );
    assert_eq!(cleaner["skipped"]["room_for_the_next_build"], 1, "{report}");
    assert!(
        idle_build.is_file(),
        "a pass under the rule took agent output: {report}"
    );
    assert_eq!(
        report["cleaners"]["cargo_build_dirs"]["deleted_items"], 0,
        "{report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
}

#[test]
fn cargo_checkouts_stay_while_a_build_runs() {
    let native = Native::new("package-caches-build");
    let checkout = native
        .home
        .join(".cargo/git/checkouts/example-0123/abcdef0/src/lib.rs");
    let npm = native.home.join(".npm/_cacache/content-v2/blob");
    file(&checkout, 8 * 1024 * 1024);
    file(&npm, 2 * 1024 * 1024);
    // A build compiling between two crates: a process named `cargo`, holding
    // nothing under the checkouts open.
    let bin = native.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink("/bin/sleep", bin.join("cargo")).unwrap();
    let mut build = Command::new(bin.join("cargo"))
        .arg("600")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    build.kill().unwrap();
    build.wait().unwrap();
    let cleaner = &report["cleaners"]["package_caches"];
    assert!(
        checkout.is_file(),
        "a git checkout went while a build was running: {report}"
    );
    assert_eq!(cleaner["skipped"]["build_running"], 1, "{report}");
    assert!(!npm.exists(), "the cache no build reads was kept: {report}");
    assert!(
        !native.home.join(".npm/._cacache.stado-removing").exists(),
        "the cache moved aside was not removed: {report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
}

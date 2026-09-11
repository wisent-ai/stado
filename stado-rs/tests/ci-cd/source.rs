//! A committed tree of more files than a release payload may carry still
//! builds, publishes and installs, and its snapshot is made of its files.
//!
//! `git archive` writes one entry per directory beside the files, and the
//! build worker used to unpack the snapshot under the bound sized for a
//! release archive - 4,096 entries. On 2026-09-10 the module split pushed
//! `wisent-ai/stado` to 3,255 files in 975 directories, every installed
//! worker refused the 0.20.9 snapshot with `release archive exceeds 4096
//! entries`, and the coordinate stayed bound to a commit nothing could unpack.

use super::*;

/// More files than the release bound admits, spread over directories so the
/// tree's `git archive` lists far more entries than it has files.
const COMMITTED_FILES: usize = 4_200;
const FILES_PER_DIRECTORY: usize = 40;
/// The entry bound a release archive is unpacked under; a snapshot of more
/// files than this used to be refused with it.
const RELEASE_ARCHIVE_ENTRY_BOUND: usize = 4_096;

/// Every regular file the committed tree carries, as `git ls-files` names it.
fn committed_paths(source: &Path) -> Vec<String> {
    let listing = run(Command::new("git").current_dir(source).args(["ls-files"])).stdout;
    let mut paths: Vec<String> = String::from_utf8(listing)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    paths.sort();
    paths
}

/// The source snapshot object the run record names, as the local store
/// lays out one `stado://<namespace>/<key>` object.
fn stored_snapshot(storage: &Path, release: &Value) -> PathBuf {
    let uri = release["source_uri"]
        .as_str()
        .unwrap_or_else(|| panic!("the run record names no source_uri: {release}"));
    let coordinate = uri
        .strip_prefix("stado://")
        .unwrap_or_else(|| panic!("the run record's source_uri is not a stado URI: {uri}"));
    let path = storage.join("ecosystem").join(coordinate);
    assert!(
        path.is_file(),
        "the run record names {uri}, and the store holds no object at {}",
        path.display()
    );
    path
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_tree_of_more_files_than_a_release_may_carry_builds_from_a_snapshot_of_its_files() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-source-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");
    for index in 0..COMMITTED_FILES {
        let directory = source
            .join("data")
            .join(format!("group-{}", index / FILES_PER_DIRECTORY));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("item-{index}.txt")),
            format!("item {index}\n"),
        )
        .unwrap();
    }
    git(&source, &["add", "data"]);
    git(
        &source,
        &["commit", "-qm", "a tree wider than a release payload"],
    );
    let committed = committed_paths(&source);
    assert!(
        committed.len() > RELEASE_ARCHIVE_ENTRY_BOUND,
        "the fixture tree carries only {} files",
        committed.len()
    );
    let archive_entries =
        run(Command::new("git")
            .current_dir(&source)
            .args(["archive", "--format=tar", "HEAD"]))
        .stdout;
    let listed = tar::Archive::new(&archive_entries[..])
        .entries()
        .unwrap()
        .count();
    assert!(
        listed > committed.len(),
        "git archive lists {listed} entries for {} files",
        committed.len()
    );

    let private = home.path().join("release-private");
    let public = home.path().join("release-public");
    let worker_bin = home.path().join(".stado/bin/stado");
    fs::create_dir_all(worker_bin.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_stado"), &worker_bin).unwrap();
    fs::set_permissions(&worker_bin, fs::Permissions::from_mode(0o700)).unwrap();
    run(Command::new(env!("CARGO_BIN_EXE_stado")).args([
        "release",
        "keygen",
        "--private-key",
        private.to_str().unwrap(),
        "--public-key",
        public.to_str().unwrap(),
        "--key-id",
        "ci-release-key",
    ]));
    let public_key = fs::read_to_string(&public).unwrap();
    let vault = SkarbiecFixture::start_release(home.path(), &private);
    registry(home.path(), &storage, &public_key, platform, None);

    let mut agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut agent_command, home.path(), &storage, &vault);
    let mut agent = Running(
        agent_command
            .args(["agent", "--target", "ci-runner"])
            .stdout(File::create(home.path().join("agent.out")).unwrap())
            .stderr(File::create(home.path().join("agent.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut agent.0);

    let mut submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit_command, home.path(), &storage, &vault);
    let mut submit = Running(
        submit_command
            .args([
                "release",
                "submit",
                "--source",
                source.to_str().unwrap(),
                "--version",
                "1.0.0",
                "--channel",
                "candidate",
                "--json",
            ])
            .stdout(File::create(home.path().join("submit.out")).unwrap())
            .stderr(File::create(home.path().join("submit.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    let status = wait_for_submit(&mut submit.0, &mut agent.0, home.path(), &storage);
    drop(submit);
    drop(agent);
    let stdout = fs::read(home.path().join("submit.out")).unwrap();
    assert!(
        status.success(),
        "a tree of {} files failed to release: {}\n{}",
        committed.len(),
        String::from_utf8_lossy(&stdout),
        fs::read_to_string(home.path().join("submit.err")).unwrap_or_default()
    );
    let release: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(release["state"], "completed", "{release}");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"], "passed",
        "{release}"
    );
    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert_eq!(
        run(&mut Command::new(installed)).stdout,
        b"ci-release-probe 1.0.0\n"
    );

    // The stored snapshot is the committed files, each once, and nothing
    // else: no directory entries, no header naming the commit.
    let snapshot = stored_snapshot(&storage, &release);
    let decoder = flate2::read::GzDecoder::new(File::open(&snapshot).unwrap());
    let mut archive = tar::Archive::new(decoder);
    let mut carried = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let kind = entry.header().entry_type();
        let path = entry.path().unwrap().display().to_string();
        assert!(
            kind.is_file(),
            "the snapshot carries {path} as {kind:?}, not as a regular file"
        );
        carried.push(path);
    }
    carried.sort();
    assert_eq!(
        carried, committed,
        "the snapshot's files differ from the committed tree"
    );
    println!(
        "verified a {}-file snapshot ({listed} archive entries) built on {platform}; evidence \
         retained at {}",
        committed.len(),
        home.keep().display()
    );
}

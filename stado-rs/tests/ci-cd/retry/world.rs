//! One isolated release world per journey: its own run root under this
//! package build directory, the operator toolchain linked in so a build can
//! actually compile, an empty store, and the fixture product committed source.

use super::super::*;

/// One isolated release world: an ignored run root under this package's own
/// `target/`, the operator's toolchain symlinked in so a build can compile,
/// an empty store and the fixture product's committed source.
pub(super) fn workspace(prefix: &str, platform: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(run_root)
        .unwrap();
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for directory in [".cargo", ".rustup"] {
        std::os::unix::fs::symlink(operator_home.join(directory), home.path().join(directory))
            .unwrap();
    }
    let storage = home.path().join("store");
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");
    (home, storage, source)
}

/// The signing key, the vault that holds it and the registry that trusts it.
/// `with_worker` also stages the built binary where a claimed job runs it,
/// which a journey that never reaches a build does not need.
pub(super) fn signed_fleet(home: &Path, storage: &Path, platform: &str, with_worker: bool) -> SkarbiecFixture {
    let private = home.join("release-private");
    let public = home.join("release-public");
    if with_worker {
        let worker_bin = home.join(".stado/bin/stado");
        fs::create_dir_all(worker_bin.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_stado"), &worker_bin).unwrap();
        fs::set_permissions(&worker_bin, fs::Permissions::from_mode(0o700)).unwrap();
    }
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
    let vault = SkarbiecFixture::start_release(home, &private);
    registry(home, storage, &public_key, platform, None, &vault.url());
    vault
}

/// A real worker, running until its capacity is claimable.
pub(super) fn claiming_agent(home: &Path, storage: &Path, vault: &SkarbiecFixture) -> Running {
    let out = File::create(home.join("agent.out")).unwrap();
    let err = File::create(home.join("agent.err")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    let mut agent = Running(
        command
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(storage, home, &mut agent.0);
    agent
}

/// One `release submit`, spawned with its output retained beside the run.
pub(super) fn submit(
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
    source: &Path,
    name: &str,
) -> Child {
    let out = File::create(home.join(format!("{name}.out"))).unwrap();
    let err = File::create(home.join(format!("{name}.err"))).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    command
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
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .unwrap()
}

pub(super) fn said_by(home: &Path, name: &str) -> String {
    format!(
        "{}{}",
        fs::read_to_string(home.join(format!("{name}.out"))).unwrap_or_default(),
        fs::read_to_string(home.join(format!("{name}.err"))).unwrap_or_default()
    )
}

/// Declare one more key on this platform's recipe and commit it, the way a
/// product declares a requirement its release worker is meant to read.
pub(super) fn declare(source: &Path, platform: &str, key: &str, value: Value) {
    let manifest_path = source.join(".wisent-release.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["platforms"][platform][key] = value;
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    run(Command::new("git")
        .current_dir(source)
        .args(["commit", "-qam", "declare a recipe key"]));
}

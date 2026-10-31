//! A release installed over the running service by anything other than its
//! own product sync pass (a `stado product install` or `finish` run by hand)
//! ends the service within one health interval, so its init system starts
//! the installed release instead of leaving the old code running until the
//! next sync period.
//!
//! One isolated deployment, as in `broken_resolver.rs`: a copy of the built
//! Stado inside it, run as `stado serve` with product sync on a period far
//! longer than the test and the health interval the test names. Its workspace
//! is a file, so the sync pass at start clones and builds nothing. After that
//! pass a new file is renamed over the program, as an installer places a
//! release. Expected: the service says a new release is installed over its
//! program, naming it, and ends with the retry exit its init system answers
//! by starting the installed release, before another sync period.
use std::io::{BufRead, BufReader, Lines};
use std::path::PathBuf;
use std::process::{ChildStderr, Command, Stdio};
use std::time::{Duration, Instant};

const TARGET: &str = "marker-host";
const PASS: &str = "[stado serve product-sync]";
const INSTALLED: &str = "a new release is installed over";

struct Deployment {
    root: PathBuf,
    program: PathBuf,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/delivery-replaced-program")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        std::fs::create_dir_all(root.join(".stado/products")).unwrap();
        std::fs::write(root.join(".stado/products/workspace"), b"not a directory\n").unwrap();
        let program = root.join("stado");
        std::fs::copy(env!("CARGO_BIN_EXE_stado"), &program).unwrap();
        Self { root, program }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH names the tools"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn cli(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().unwrap();
        assert!(output.status.success(), "stado {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The shared directory template with this machine as `marker-host`.
    fn import_registry(&self) {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"].as_array_mut().and_then(|targets| targets.first_mut()).unwrap();
        let hostname = Command::new("hostname").output().unwrap();
        let hostname = String::from_utf8(hostname.stdout).unwrap().trim().to_lowercase();
        target["hostnames"] = serde_json::json!([hostname]);
        let printed = self.cli(&["host", "local", "port"]);
        target["service_resolver"]["api_bind"] = serde_json::json!(format!("127.0.0.1:{}", printed.trim()));
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    /// Place a release over the program the way an installer does: a new
    /// file renamed over it, so the running process keeps its old file.
    fn install_over(&self) {
        let staged = self.root.join("stado.new");
        std::fs::copy(&self.program, &staged).unwrap();
        let mut bytes = std::fs::read(&staged).unwrap();
        bytes.extend_from_slice(b"\n");
        std::fs::write(&staged, bytes).unwrap();
        std::fs::rename(&staged, &self.program).unwrap();
    }
}

fn read_until(lines: &mut Lines<BufReader<ChildStderr>>, marker: &str, said: &mut Vec<String>) -> String {
    loop {
        let Some(line) = lines.next() else {
            panic!("the service ended before saying {marker:?}: {said:#?}");
        };
        let line = line.unwrap();
        said.push(line.clone());
        if line.contains(marker) {
            return line;
        }
    }
}

fn seconds(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} names a number of seconds"))
}

#[test]
#[ignore = "requires STADO_TEST_SYNC_INTERVAL_SECONDS (far longer than the test) and STADO_TEST_HEALTH_INTERVAL_SECONDS"]
fn a_release_installed_by_hand_ends_the_service_within_one_health_interval() {
    let sync = seconds("STADO_TEST_SYNC_INTERVAL_SECONDS");
    let health = seconds("STADO_TEST_HEALTH_INTERVAL_SECONDS");
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let mut service = deployment
        .command()
        .args([
            "serve",
            "--target",
            TARGET,
            "--health-interval-seconds",
            &health,
            "--product-sync-interval-seconds",
            &sync,
            "--product-sync-surface",
            "cli",
        ])
        .stdout(std::fs::File::create(deployment.root.join("service.stdout")).unwrap())
        .spawn()
        .unwrap();
    let mut said = Vec::new();
    let mut lines = BufReader::new(service.stderr.take().unwrap()).lines();

    // The pass at start ran; the next one is a whole sync period away.
    read_until(&mut lines, PASS, &mut said);
    let placed = Instant::now();
    deployment.install_over();
    let ended = read_until(&mut lines, INSTALLED, &mut said);
    let took = placed.elapsed();
    assert!(
        ended.contains(&deployment.program.to_string_lossy().into_owned()),
        "the end names the program the release was installed over: {ended}"
    );
    said.extend(lines.map(|line| line.unwrap()));
    let status = service.wait().unwrap();
    std::fs::write(deployment.root.join("service.stderr"), said.join("\n")).unwrap();
    // A replaced service ends with the retry exit its init system answers
    // by starting the installed release; its delivery is work done, never a
    // stop of this release.
    assert!(!status.success(), "a replaced service ends with the retry exit, not success: {said:#?}");
    assert!(
        said.iter().any(|line| line.contains("component=product-sync") && line.contains("finished its work")),
        "product sync ended as finished work: {said:#?}"
    );
    assert!(!said.iter().any(|line| line.contains("installed nothing")), "the end was read as a stop: {said:#?}");
    let sync: u64 = sync.parse().unwrap();
    assert!(took < Duration::from_secs(sync), "the end waited for a sync period ({took:?}): {said:#?}");
}

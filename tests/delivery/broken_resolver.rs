//! A host whose resolver cannot start still runs product sync, and ends its
//! service once a release is installed over the program it runs, so the init
//! system starts the repair.
//!
//! One isolated deployment, as in `directory_connect/not_serving.rs`: a local
//! store holding the shared directory template with this machine as
//! `marker-host`, and this test holding the resolution API's bind, so the real
//! resolver of `stado serve --resolver` fails. The service is a copy of the
//! built Stado inside the deployment, run with product sync. Its workspace is
//! a file, so every product is held at once and no product is cloned or built
//! on the person's network. Expected: the service says it failed and keeps
//! product sync running, it is still alive after a sync pass, and when the
//! built program is installed over its copy (staged beside it and renamed
//! into place, as an installer does) the next pass ends the service with the
//! cause naming the installed release.
use std::io::{BufRead, BufReader, Lines};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::PathBuf;
use std::process::{ChildStderr, Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
const KEEPS: &str = "keeps product-sync running";
const INSTALLED: &str = "installed a new release over";
const PASS: &str = "[stado serve product-sync]";

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
            .join(".build/delivery-broken-resolver")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        std::fs::create_dir_all(root.join(".stado/products")).unwrap();
        // A file where the workspace directory belongs: every product's
        // checkout is refused at once, so the pass clones and builds nothing.
        std::fs::write(root.join(".stado/products/workspace"), b"not a directory\n").unwrap();
        let program = root.join("stado");
        std::fs::copy(env!("CARGO_BIN_EXE_stado"), &program).unwrap();
        Self { root, program }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
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
        let output: Output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    fn import_registry(&self, api_bind: &str) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

/// The service's stderr up to the first line holding `marker`, kept in `said`.
fn read_until(
    lines: &mut Lines<BufReader<ChildStderr>>,
    marker: &str,
    said: &mut Vec<String>,
) -> String {
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

#[test]
#[ignore = "requires STADO_TEST_SYNC_INTERVAL_SECONDS, the seconds between product sync passes"]
fn product_sync_outlives_a_broken_resolver_and_restarts_into_the_installed_release() {
    let interval = std::env::var("STADO_TEST_SYNC_INTERVAL_SECONDS")
        .expect("STADO_TEST_SYNC_INTERVAL_SECONDS names the seconds between sync passes");
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    deployment.import_registry(&api_bind);

    let holder = TcpListener::bind(&api_bind).expect("hold the resolution API's bind");
    let mut service = deployment
        .command()
        .args([
            "serve",
            "--resolver",
            "--target",
            TARGET,
            "--product-sync-interval-seconds",
            &interval,
            "--product-sync-surface",
            "cli",
        ])
        // Each pass prints its rows; a pipe nobody reads would stall it.
        .stdout(std::fs::File::create(deployment.root.join("service.stdout")).unwrap())
        .spawn()
        .unwrap();
    let mut said = Vec::new();
    let mut lines = BufReader::new(service.stderr.take().unwrap()).lines();

    let failed = read_until(&mut lines, KEEPS, &mut said);
    assert!(
        failed.contains(&api_bind),
        "the resolver's own cause, the bind it could not take, is named: {failed}"
    );
    read_until(&mut lines, PASS, &mut said);
    assert!(
        service.try_wait().unwrap().is_none(),
        "the service ended with its resolver although product sync was running: {said:#?}"
    );

    // Install the built program over the copy the service runs.
    let staged = deployment.root.join("stado.installing");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &staged).unwrap();
    std::fs::rename(&staged, &deployment.program).unwrap();

    let ended = read_until(&mut lines, INSTALLED, &mut said);
    assert!(
        ended.contains(&deployment.program.to_string_lossy().into_owned()),
        "the end names the program the release was installed over: {ended}"
    );
    let status = service.wait().unwrap();
    assert!(
        !status.success(),
        "the service ends as a failure, so its init system starts it again: {said:#?}"
    );
    drop(holder);
    std::fs::write(deployment.root.join("service.stderr"), said.join("\n")).unwrap();
}

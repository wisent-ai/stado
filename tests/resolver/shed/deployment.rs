//! One isolated deployment for the shed journey: a local store with this
//! machine as `marker-host` and as the host of `elsewhere-api`, one adapter,
//! the declared refresh interval, and the real `stado serve --resolver`
//! started under a descriptor limit with its stderr kept.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

pub(crate) const TARGET: &str = "marker-host";
pub(crate) const SERVICE: &str = "elsewhere-api";
pub(crate) const CONSUMER: &str = "held-consumer";
/// The line `stado serve --resolver` prints once it publishes `serving`.
const SERVING: &str = "stado resolver target=";

pub(crate) struct Deployment {
    pub(crate) root: PathBuf,
    resolver: Option<Child>,
}

impl Deployment {
    pub(crate) fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/resolver-shed")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            resolver: None,
        }
    }

    fn environment(&self, command: &mut Command) {
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
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        self.environment(&mut command);
        command
    }

    pub(crate) fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    pub(crate) fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A loopback address this machine just handed out and nothing holds now.
    pub(crate) fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader and as
    /// the host of `elsewhere-api`, served at `service_bind`, one adapter at
    /// `adapter_bind`, and the declared refresh interval.
    pub(crate) fn import_registry(
        &self,
        api_bind: &str,
        adapter_bind: &str,
        service_bind: &str,
        refresh_seconds: u64,
    ) {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        target["service_resolver"]["refresh_seconds"] = json!(refresh_seconds);
        target["service_resolver"]["adapters"] =
            json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
        let service = &mut document["service_directory"]["services"][SERVICE];
        service["active_host"] = json!(TARGET);
        service["endpoints"][TARGET] = json!({"url": format!("http://{service_bind}")});
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    /// Start the real resolver under `fd_limit` descriptors and return once
    /// it says it serves; its stderr keeps being read into a file.
    pub(crate) fn serve(&mut self, fd_limit: u64) -> std::thread::JoinHandle<Vec<String>> {
        let mut command = Command::new("sh");
        self.environment(&mut command);
        command.arg("-c").arg(format!(
            "ulimit -n {fd_limit} && exec \"$0\" serve --resolver --target {TARGET}"
        ));
        command.arg(env!("CARGO_BIN_EXE_stado"));
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().expect("the resolver's stderr is piped");
        let mut lines = BufReader::new(stderr).lines();
        let mut said = Vec::new();
        for line in lines.by_ref() {
            let line = line.expect("the resolver's stderr reads");
            said.push(line.clone());
            if line.starts_with(SERVING) {
                break;
            }
        }
        assert!(
            said.iter().any(|line| line.starts_with(SERVING)),
            "the resolver ended without serving: {}",
            said.join("\n")
        );
        self.resolver = Some(child);
        let log = self.root.join("resolver.stderr");
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                said.push(line);
                std::fs::write(&log, said.join("\n")).unwrap();
            }
            said
        })
    }

    pub(crate) fn published_state(&self) -> Value {
        let path = self.root.join(".stado").join("resolver-state.json");
        serde_json::from_slice(&std::fs::read(&path).expect("the resolver published its state"))
            .expect("the published state is JSON")
    }

    /// Ask `stado resolver status` until it reports the adapter's waits held
    /// past the refresh interval: the product's own word that the first
    /// wave has aged, each ask a real run of the command.
    pub(crate) fn until_held(&self, adapter_bind: &str) -> String {
        loop {
            let output = self.run(&["resolver", "status", "--target", TARGET, "--json"]);
            let reported = String::from_utf8_lossy(&output.stdout).into_owned();
            let status: Value = match serde_json::from_str(&reported) {
                Ok(status) => status,
                Err(error) => panic!("resolver status printed {reported:?}: {error}"),
            };
            let held = status["blockers"].as_array().is_some_and(|blockers| {
                blockers.iter().any(|blocker| {
                    blocker
                        .as_str()
                        .is_some_and(|text| text.contains(adapter_bind))
                })
            });
            if held {
                return reported;
            }
        }
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(mut child) = self.resolver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

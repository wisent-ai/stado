//! The isolated deployment `held.rs` drives: a local store, the real stado
//! binary with `HOME` inside the checkout's build directory, a resolver
//! whose log is followed line by line, and a report of every command kept
//! beside it, bound to the checkout's revision. Every port is one `stado
//! host free-port-local` hands out.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Lines};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Output, Stdio};

pub(crate) const TARGET: &str = "marker-host";
pub(crate) const SERVICE: &str = "marker-api";
pub(crate) const CONSUMER: &str = "marker-consumer";
const WAITING_FILE: &str = "resolver-waiting.json";

/// The revision of the checkout the test binary was built from.
fn source_revision(repository: &std::path::Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git answers the checkout's revision");
    String::from_utf8(output.stdout)
        .expect("a git revision is text")
        .trim()
        .to_string()
}

pub(crate) struct Deployment {
    pub(crate) root: PathBuf,
    pub(crate) report: Value,
    resolver: Option<(Child, Lines<BufReader<ChildStderr>>)>,
}

impl Deployment {
    pub(crate) fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/service-directory-held")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        Self {
            report: json!({
                "source_revision": source_revision(&repository),
                "commands": [],
                "resolver_log": [],
                "outcome": "failed",
            }),
            root,
            resolver: None,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
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
            .stdin(Stdio::null());
        command
    }

    fn run_command(&mut self, mut command: Command, args: &[&str]) -> Output {
        let output = command.args(args).output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    pub(crate) fn run(&mut self, args: &[&str]) -> Output {
        let command = self.command();
        self.run_command(command, args)
    }

    pub(crate) fn cli(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&mut self) -> u16 {
        let printed = self.cli(&["host", "free-port-local"]);
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        }
    }

    /// A loopback address this machine just handed out and nothing holds now.
    pub(crate) fn free_bind(&mut self) -> String {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.free_port()).to_string()
    }

    /// A service that accepts every connection and answers none, held open
    /// for as long as the test runs, at an address this machine handed out.
    pub(crate) fn silent_service(&mut self) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port())).unwrap();
        let bind = listener.local_addr().unwrap().to_string();
        let holder = std::thread::spawn(move || {
            let mut held = Vec::new();
            for accepted in listener.incoming() {
                match accepted {
                    Ok(stream) => held.push(stream),
                    Err(_) => return,
                }
            }
        });
        (bind, holder)
    }

    /// The same command with its store being the service behind the
    /// adapter, as a reader whose registry lives behind the fleet's object
    /// API is configured.
    pub(crate) fn through_adapter(&mut self, args: &[&str]) -> Output {
        let token = self.root.join("adapter-token");
        fs::write(&token, "marker-token\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let owner_only = stado::primitives::file_mode::owner_read_write();
            fs::set_permissions(&token, fs::Permissions::from_mode(owner_only)).unwrap();
        }
        let mut command = self.command();
        command
            .env("WC_STORAGE_BACKEND", "stado")
            .env(
                "WC_STADO_STORAGE_URL",
                format!("stado://service/{SERVICE}?consumer={CONSUMER}"),
            )
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .env("WC_STADO_STORAGE_NAMESPACE", "marker-namespace");
        self.run_command(command, args)
    }

    pub(crate) fn serve(&mut self) {
        let mut child = self
            .command()
            .args(["serve", "--resolver", "--target", TARGET])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stderr.take().unwrap()).lines();
        self.resolver = Some((child, lines));
    }

    /// Read the resolver's log until a line contains `wanted`; the resolver
    /// exiting first ends the test with everything it said.
    pub(crate) fn await_line(&mut self, wanted: &str) -> String {
        loop {
            let line = self.resolver.as_mut().unwrap().1.next();
            let Some(Ok(line)) = line else {
                self.save();
                panic!(
                    "the resolver stopped before saying {wanted:?}: {}",
                    self.report["resolver_log"]
                );
            };
            self.report["resolver_log"]
                .as_array_mut()
                .unwrap()
                .push(json!(line));
            if line.contains(wanted) {
                self.save();
                return line;
            }
        }
    }

    /// The waits the resolver published beside its state.
    pub(crate) fn published_waits(&self) -> Vec<Value> {
        let path = self.root.join(".stado").join(WAITING_FILE);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) => panic!("{} is not published: {error}", path.display()),
        };
        serde_json::from_str(&body).unwrap()
    }

    pub(crate) fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.resolver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

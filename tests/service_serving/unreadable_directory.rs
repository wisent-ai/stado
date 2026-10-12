//! `stado service serving <name> --host <host>` whose service directory
//! cannot be read is refused with the directory's own failure, naming the
//! host and the cause; it does not read the failure as a service that
//! declares no endpoint and ask for `--port`, and it does not answer as if
//! the service were simply not registry-managed.
//!
//! The real stado runs with an isolated `HOME` under the checkout's build
//! directory, no last-known-good registry copy, and a fleet store that is a
//! loopback listener this test owns, which resets every connection after its
//! request line, so every directory read fails with the transport's cause.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SERVICE: &str = "marker-api";
const HOST: &str = "marker-host";
const UNREAD: &str = "could not be read";
const NO_ENDPOINT: &str = "declares no endpoint";
const ASKS_PORT: &str = "--port";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("service-serving-unreadable");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(root.join("tmp")).expect("create the run directory");
        Self { root }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let output = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The fleet store every directory read goes to: a loopback listener
    /// that resets every connection after its request line.
    fn resetting_store(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut request_line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut request_line);
                drop(stream);
            }
        });
        address
    }

    fn serving(&self, store: &str) -> Output {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "unreadable-directory-probe").unwrap();
        let output = self
            .stado()
            .args(["service", "serving", SERVICE, "--host", HOST])
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("service-serving.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn an_unreadable_directory_is_the_refusal_not_a_service_with_no_port() {
    let run = Run::new();
    let store = run.resetting_store();
    let output = run.serving(&store);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a serving check whose directory cannot be read is refused: {said}"
    );
    assert!(
        said.contains(UNREAD) && said.contains(HOST),
        "the refusal says the directory could not be read and names the host: {said}"
    );
    assert!(
        !said.contains(NO_ENDPOINT) && !said.contains(ASKS_PORT),
        "a directory that could not be read is not reported as a service with no declared port: {said}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "",
        "no serving report is printed for a check that could not read the directory"
    );
}

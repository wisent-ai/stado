//! The isolated run of the client grant journey: the real `stado` and
//! `skarbiec` under one build-directory `HOME`, the owner-side layout, the
//! resetting registry store, and the command log kept beside the run.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

use super::{CONSUMER, DATABASE, ITEM};

/// The mode the product requires of a store token file: read and write by
/// its owner alone, so the store is reached and does not answer.
const OWNER_READ_WRITE: u32 = (nix::libc::S_IRUSR | nix::libc::S_IWUSR) as u32;

pub(crate) struct Run {
    pub(crate) root: PathBuf,
    store: String,
    commands: std::cell::RefCell<Vec<Value>>,
}

impl Run {
    pub(crate) fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/database-client-grant")
            .join(format!("{name}-{}", std::process::id()));
        for directory in [".stado/cache", ".stado/bin", ".config/stado", "tmp", "g"] {
            std::fs::create_dir_all(root.join(directory)).expect("create the run directory");
        }
        let skarbiec =
            std::env::var("SKARBIEC_BIN").expect("SKARBIEC_BIN names the Skarbiec under test");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), root.join(".stado/bin/stado"))
            .expect("install the stado under test as the owner's");
        std::os::unix::fs::symlink(skarbiec, root.join(".stado/bin/skarbiec"))
            .expect("install the skarbiec under test as the owner's");
        let token = root.join("object-api.token");
        std::fs::write(&token, "database-client-grant-probe").expect("write the store token");
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(OWNER_READ_WRITE))
            .expect("make the store token owner-only");
        let mut run = Self {
            root,
            store: String::new(),
            commands: std::cell::RefCell::new(Vec::new()),
        };
        run.store = run.resetting_store();
        run
    }

    fn environment(&self, command: &mut Command) {
        command
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the programs stado runs"),
            )
            .env("HOME", &self.root)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", self.root.join(".config/stado/config.json"))
            .env("GNUPGHOME", self.root.join("g"))
            .env("SKARBIEC_VAULT_FILE", self.root.join("vault.json"))
            .env("SKARBIEC_AUDIT_FILE", self.root.join("audit.jsonl"))
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{}", self.store))
            .env(
                "WC_STADO_STORAGE_TOKEN_FILE",
                self.root.join("object-api.token"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let printed = Self::succeeded(&self.stado(&["host", "local", "port"]));
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The registry store: a loopback listener that resets every connection
    /// after its request line, so no registry read is answered.
    fn resetting_store(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener
            .local_addr()
            .expect("the bound listener has an address")
            .to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(stream) => stream,
                    Err(error) => panic!("the resetting store stopped accepting: {error}"),
                };
                let mut request_line = String::new();
                if let Err(error) = BufReader::new(&stream).read_line(&mut request_line) {
                    eprintln!("the resetting store read no request line: {error}");
                }
                drop(stream);
            }
        });
        address
    }

    fn run(&self, command: &mut Command, input: Option<&str>) -> Output {
        self.environment(command);
        let output = match input {
            Some(text) => stado_wait::output_with_input(command, text.as_bytes()),
            None => stado_wait::output(command),
        }
        .expect("run the real program");
        self.commands.borrow_mut().push(json!({
            "program": command.get_program().to_string_lossy(),
            "args": command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "exit": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        std::fs::write(
            self.root.join("commands.json"),
            serde_json::to_vec_pretty(&*self.commands.borrow())
                .expect("the command log serializes"),
        )
        .expect("keep the commands beside the run");
        output
    }

    pub(crate) fn stado(&self, args: &[&str]) -> Output {
        self.run(Command::new(env!("CARGO_BIN_EXE_stado")).args(args), None)
    }

    pub(crate) fn skarbiec(&self, args: &[&str], input: Option<&str>) -> Output {
        let program =
            std::env::var("SKARBIEC_BIN").expect("SKARBIEC_BIN names the Skarbiec under test");
        self.run(Command::new(program).args(args), input)
    }

    pub(crate) fn succeeded(output: &Output) -> String {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout.clone()).expect("UTF-8 output")
    }

    /// The shared directory template with this machine as its first target
    /// and Skarbiec placed on it, so this machine is the vault owner.
    pub(crate) fn place_vault_here(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json"))
                .expect("the shared directory template is JSON");
        let here = document["targets"][0]["name"]
            .as_str()
            .expect("the first target has a name")
            .to_string();
        document["targets"][0]["hostnames"] = json!([hostname()]);
        document["service_directory"]["services"]["skarbiec"] = json!({
            "managed_service": "skarbiec", "active_host": here, "endpoints": {}, "consumers": {},
        });
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).expect("the copy serializes"),
        )
        .expect("write the last-known-good copy");
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            json!({"read_at": chrono::Utc::now().to_rfc3339(), "generation": "example"})
                .to_string(),
        )
        .expect("write the last-known-good metadata");
    }

    pub(crate) fn store_item(&self, fields: &Value) {
        let envelope = json!({ "schema": "skarbiec.item.v2", "kind": "bundle", "context": {}, "fields": fields });
        Self::succeeded(&self.skarbiec(&["set", ITEM], Some(&envelope.to_string())));
    }

    pub(crate) fn client(&self) -> Output {
        let declared =
            json!({DATABASE: {"engine": "postgres", "scopes": ["read"], "consumers": ["reader"]}});
        self.run(
            Command::new(env!("CARGO_BIN_EXE_stado"))
                .args(["database", "client", DATABASE, "--json"])
                .env("WC_DATABASE_API_DATABASES", declared.to_string()),
            None,
        )
    }

    /// The fields the vault's stored grant lets the client read on the item.
    pub(crate) fn granted_fields(&self) -> Vec<String> {
        let grants: Value =
            serde_json::from_str(&Self::succeeded(&self.skarbiec(&["grant", "list"], None)))
                .expect("grant list JSON");
        let mut fields: Vec<String> = grants
            .as_array()
            .expect("grants are a list")
            .iter()
            .filter(|grant| grant["consumer"] == CONSUMER)
            .flat_map(|grant| {
                grant["capabilities"]
                    .as_array()
                    .expect("capabilities are a list")
                    .clone()
            })
            .filter(|capability| capability["item"] == ITEM)
            .filter_map(|capability| capability["field"].as_str().map(str::to_string))
            .collect();
        fields.sort();
        fields
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let stopped = Command::new("gpgconf")
            .args(["--kill", "all"])
            .env("GNUPGHOME", self.root.join("g"))
            .status();
        if !matches!(stopped, Ok(status) if status.success()) {
            eprintln!(
                "the run's GPG agent was not confirmed stopped; {} is kept",
                self.root.display()
            );
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!(
                "the run directory {} was not removed: {error}",
                self.root.display()
            );
        }
    }
}

fn hostname() -> String {
    let output =
        stado_wait::output(&mut Command::new("hostname")).expect("run this machine's hostname");
    String::from_utf8(output.stdout)
        .expect("the hostname is UTF-8")
        .trim()
        .to_lowercase()
}

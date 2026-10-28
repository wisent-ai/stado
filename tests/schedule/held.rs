//! A host declares its schedules while the fleet store does not answer:
//! `stado schedule create` keeps the declaration on this host and says so,
//! `stado schedule list` shows it held and names the store's refusal, and
//! the first schedule command that reaches a store writes it there and
//! removes the held file. A declaration the store refuses stays held.
//!
//! The real `stado` runs against a store that answers every request with a
//! gateway error, then against an isolated local store under the build
//! directory; every verdict is read back from the files and the store.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use reqwest::StatusCode;
use serde_json::Value;

const CRON: &str = "* * * * *";
const HELD_COMMAND: &str = "echo declared while the store was down";
const OTHER_COMMAND: &str = "echo another declaration under the same id";

struct Host {
    root: PathBuf,
    id: String,
    down: String,
}

impl Host {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/journeys/schedule-held");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        std::fs::write(root.join("object-api.token"), "schedule-held-probe")
            .expect("write the store token");
        let mut host = Self {
            root,
            id: uuid::Uuid::new_v4().to_string(),
            down: String::new(),
        };
        host.down = host.unanswering_store();
        host
    }

    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A free loopback port, as the real stado finds one.
    fn free_port(&self) -> u16 {
        let output = stado_wait::output(self.stado().args(["host", "local", "port"]))
            .expect("run the real stado");
        let printed = text(&output.stdout);
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// A loopback store that answers every request with a gateway error, as
    /// the fleet's object API relay answers with no store behind it; answers
    /// the address the real stado is pointed at.
    fn unanswering_store(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the unanswering store");
        let address = listener
            .local_addr()
            .expect("the store's address")
            .to_string();
        let answer = format!(
            "HTTP/1.1 {}\r\nconnection: close\r\n\r\n",
            StatusCode::BAD_GATEWAY
        );
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    continue;
                };
                let request = BufReader::new(connection.try_clone().expect("clone the stream"));
                // The request's head ends at its first empty line.
                for line in request.lines() {
                    match line {
                        Ok(line) if !line.is_empty() => continue,
                        _ => break,
                    }
                }
                connection
                    .write_all(answer.as_bytes())
                    .expect("answer the gateway error");
            }
        });
        address
    }

    /// Run against the store that does not answer.
    fn while_down(&self, arguments: &[&str]) -> Output {
        stado_wait::output(
            self.stado()
                .args(arguments)
                .env("WC_STORAGE_BACKEND", "stado")
                .env("WC_STADO_STORAGE_URL", format!("http://{}", self.down))
                .env(
                    "WC_STADO_STORAGE_TOKEN_FILE",
                    self.root.join("object-api.token"),
                ),
        )
        .expect("run the real stado")
    }

    /// Run against the isolated local store.
    fn while_up(&self, arguments: &[&str]) -> Output {
        stado_wait::output(
            self.stado()
                .args(arguments)
                .env("WC_STORAGE_BACKEND", "local")
                .env("WC_LOCAL_STORAGE_PATH", self.root.join("store")),
        )
        .expect("run the real stado")
    }

    fn declare(&self, command: &str) -> Vec<String> {
        [
            "schedule",
            "create",
            "--id",
            self.id.as_str(),
            "--cron",
            CRON,
            command,
        ]
        .map(String::from)
        .to_vec()
    }

    fn held_file(&self) -> PathBuf {
        let stored = format!(
            "sch-{}",
            uuid::Uuid::parse_str(&self.id).expect("a uuid").simple()
        );
        self.root
            .join(".stado/schedules/held")
            .join(format!("{stored}.json"))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!(
                "the run directory {} was not removed: {error}",
                self.root.display()
            );
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn strings(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

#[test]
fn a_declaration_the_store_cannot_take_is_held_and_written_when_it_answers() {
    let host = Host::new("held");
    let created = host.while_down(&strings(&host.declare(HELD_COMMAND)));
    assert!(
        created.status.success(),
        "schedule create: {}",
        text(&created.stderr)
    );
    let said = text(&created.stdout);
    assert!(
        said.contains("held schedule") && said.contains("did not answer"),
        "{said}"
    );
    let held: Value = serde_json::from_slice(
        &std::fs::read(host.held_file()).expect("the held declaration is kept on this host"),
    )
    .expect("the held declaration is JSON");
    assert_eq!(held["schedule"]["command"], HELD_COMMAND);
    let gateway = StatusCode::BAD_GATEWAY.as_str().to_string();
    assert!(
        held["refusal"]
            .as_str()
            .is_some_and(|refusal| refusal.contains(&gateway)),
        "{held}"
    );

    let listed = host.while_down(&["schedule", "list"]);
    assert!(
        !listed.status.success(),
        "a list without the store's schedules is not complete"
    );
    assert!(
        text(&listed.stdout).contains("held on this host at"),
        "{}",
        text(&listed.stdout)
    );
    assert!(
        text(&listed.stderr).contains("the fleet store did not answer"),
        "{}",
        text(&listed.stderr)
    );

    let delivered = host.while_up(&["schedule", "list", "--json"]);
    assert!(
        delivered.status.success(),
        "schedule list: {}",
        text(&delivered.stderr)
    );
    assert!(
        text(&delivered.stderr).contains("written into the fleet store"),
        "{}",
        text(&delivered.stderr)
    );
    assert!(
        !host.held_file().exists(),
        "the delivered declaration is no longer held"
    );
    let rows: Vec<Value> = serde_json::from_slice(&delivered.stdout).expect("list prints JSON");
    let stored = rows
        .iter()
        .find(|row| row["command"] == HELD_COMMAND)
        .expect("the store holds the held declaration");
    assert!(
        stored.get("held").is_none(),
        "a stored schedule is not marked held: {stored}"
    );
}

#[test]
fn a_held_declaration_the_store_refuses_stays_held_and_is_said() {
    let host = Host::new("refused");
    let standing = host.while_up(&strings(&host.declare(OTHER_COMMAND)));
    assert!(
        standing.status.success(),
        "schedule create: {}",
        text(&standing.stderr)
    );
    let created = host.while_down(&strings(&host.declare(HELD_COMMAND)));
    assert!(
        created.status.success(),
        "schedule create: {}",
        text(&created.stderr)
    );
    assert!(host.held_file().exists(), "the declaration is held");

    let listed = host.while_up(&["schedule", "list"]);
    assert!(
        listed.status.success(),
        "schedule list: {}",
        text(&listed.stderr)
    );
    assert!(
        text(&listed.stderr).contains("refused it"),
        "{}",
        text(&listed.stderr)
    );
    assert!(
        host.held_file().exists(),
        "a refused declaration stays held"
    );
    let shown = host.while_up(&["schedule", "show", &host.id, "--json"]);
    let record: Value = serde_json::from_slice(&shown.stdout).expect("show prints JSON");
    assert_eq!(
        record["command"], OTHER_COMMAND,
        "the standing schedule is unchanged"
    );
}

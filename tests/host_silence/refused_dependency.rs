//! A dependency that refuses ends the command at once with that refusal,
//! and the command never sends the same request again into it.
//!
//! The real `stado storage get` runs with an isolated `HOME` under the
//! checkout's build directory and `STADO_API_URL` naming a loopback object
//! API. In the first journey nothing listens on the port: the command exits
//! non-zero naming the refused connection. In the second a listener this
//! test owns answers the GET with a declared body and closes the connection
//! after its first word: the command exits non-zero naming the cut, and the
//! listener saw exactly one request, so a cut body is not answered by asking
//! the dependency again.
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const OBJECT: &str = "stado://journey/refused-dependency.json";
const BODY: &str = "an object the listener declares whole and sends in part";
const REFUSED: &str = "the object API refused the connection";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new(journey: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/host-silence-refused-dependency");
        let root = parent.join(format!("{}-{journey}", std::process::id()));
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self { root }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the programs stado runs"),
            )
            .env("HOME", &self.root)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let output = stado_wait::output(self.stado().args(["host", "local", "port"]))
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// `stado storage get` of one object from the object API at `address`,
    /// its stderr kept beside the run.
    fn storage_get(&self, address: &str) -> Output {
        let output = stado_wait::output(
            self.stado()
                .args(["storage", "get", OBJECT, "-"])
                .env("STADO_API_URL", format!("http://{address}"))
                .env("STADO_API_TOKEN", "refused-dependency-probe"),
        )
        .expect("run the real stado");
        std::fs::write(self.root.join("storage-get.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!(
                "the run directory {} was not removed: {error}",
                self.root.display()
            );
        }
    }
}

fn first_word() -> &'static str {
    BODY.split(' ').next().expect("the body has a first word")
}

/// A loopback object API that answers every GET with `BODY`'s full length
/// declared and only its first word sent, then closes the connection. The
/// request lines it was sent are kept in order.
fn cutting_listener(port: u16) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .expect("bind the loopback port this machine handed out");
    let address = listener
        .local_addr()
        .expect("the bound listener has an address")
        .to_string();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(error) => panic!("the cutting listener stopped accepting: {error}"),
            };
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            if let Err(error) = reader.read_line(&mut request_line) {
                panic!("the cutting listener read no request line: {error}");
            }
            kept.lock()
                .expect("the request log is not poisoned")
                .push(request_line.trim().to_owned());
            // The rest of the request's head ends at its first empty line;
            // at the end of the stream the line read is empty too.
            let mut line = String::new();
            loop {
                line.clear();
                if let Err(error) = reader.read_line(&mut line) {
                    panic!("the cutting listener could not read the request head: {error}");
                }
                if line.trim().is_empty() {
                    break;
                }
            }
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                BODY.len(),
                first_word()
            );
            if let Err(error) = stream.write_all(answer.as_bytes()) {
                panic!("the cutting listener could not answer: {error}");
            }
        }
    });
    (address, requests)
}

#[test]
fn a_port_nobody_listens_on_ends_the_get_with_the_refusal() {
    let run = Run::new("closed");
    let address = format!("{}:{}", Ipv4Addr::LOCALHOST, run.free_port());
    let output = run.storage_get(&address);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a GET nobody can answer is refused, not answered: {said}"
    );
    assert!(said.contains(REFUSED), "the refusal is the cause: {said}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "",
        "no object is printed for a refused GET"
    );
}

#[test]
fn a_body_cut_short_ends_the_get_after_one_request() {
    let run = Run::new("cut");
    let (address, requests) = cutting_listener(run.free_port());
    let output = run.storage_get(&address);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a body cut short is a failed GET: {said}"
    );
    assert!(
        said.contains(&format!(
            "after {} of {} bytes",
            first_word().len(),
            BODY.len()
        )) || said.contains("connection closed before completion"),
        "the cut is the cause, with the bytes it reached: {said}"
    );
    let seen = requests.lock().expect("the request log is not poisoned");
    match seen.as_slice() {
        [only] => assert!(
            only.starts_with("GET /api/object"),
            "the one request is the object GET: {only}"
        ),
        asked => panic!(
            "the dependency is asked once; a cut body is not answered by asking again: {asked:?}"
        ),
    }
}

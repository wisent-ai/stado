//! A dependency that refuses ends the command at once with that refusal,
//! and the command never sends the same request again into it.
//!
//! The real `stado storage get` runs with an isolated `HOME` under the
//! checkout's build directory and `STADO_API_URL` naming a loopback object
//! API. In the first journey nothing listens on the port: the command exits
//! non-zero naming the refused connection. In the second a listener this
//! test owns answers every GET with a declared body and closes the
//! connection after its first word: the command asks once for the rest from
//! the byte the cut reached, is answered with the whole object again instead
//! of that range, and exits non-zero naming the refused range; the listener
//! saw exactly two requests, so a dependency that cannot serve the rest is
//! not asked again. In the third the real `stado host ping` has the closed
//! port as its beacon store's object API: it asks that store once, exits
//! non-zero naming itself, the address and the refused connection, and runs
//! no registry read and no SSH step behind it.
use std::fs::Permissions;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const OBJECT: &str = "stado://journey/refused-dependency.json";
const BODY: &str = "an object the listener declares whole and sends in part";
const REFUSED: &str = "the object API refused the connection";
const HOST: &str = "refused-dependency-host";
const NAMESPACE: &str = "refused-dependency";
const LAYOUT: &str = "system/storage-layout.json";
const OWNER_READ_WRITE: u32 = (nix::libc::S_IRUSR | nix::libc::S_IWUSR) as u32;

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

    /// `command` run to its end, its stderr kept beside the run as `name`.
    fn kept(&self, command: &mut Command, name: &str) -> Output {
        let output = stado_wait::output(command).expect("run the real stado");
        std::fs::write(self.root.join(name), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }

    /// `stado storage get` of one object from the object API at `address`.
    fn storage_get(&self, address: &str) -> Output {
        let mut command = self.stado();
        command
            .args(["storage", "get", OBJECT, "-"])
            .env("STADO_API_URL", format!("http://{address}"))
            .env("STADO_API_TOKEN", "refused-dependency-probe");
        self.kept(&mut command, "storage-get.stderr")
    }

    /// `stado host ping HOST` whose beacon store is the object API at
    /// `address`, reached with the owner-only token file that store requires.
    fn host_ping(&self, address: &str) -> Output {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "refused-dependency-probe").expect("write the store token");
        std::fs::set_permissions(&token, Permissions::from_mode(OWNER_READ_WRITE))
            .expect("make the store token owner-only");
        let mut command = self.stado();
        command
            .args(["host", "ping", HOST])
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{address}"))
            .env("WC_STADO_STORAGE_NAMESPACE", NAMESPACE)
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token);
        self.kept(&mut command, "host-ping.stderr")
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

/// One request the cutting listener was sent: its request line and the
/// `Range` header it carried, if any.
#[derive(Debug)]
struct Asked {
    line: String,
    range: Option<String>,
}

/// A loopback object API that answers every GET with `BODY`'s full length
/// declared and only its first word sent, then closes the connection. The
/// requests it was sent are kept in order, each with the range it asked.
fn cutting_listener(port: u16) -> (String, Arc<Mutex<Vec<Asked>>>) {
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
            // The rest of the request's head ends at its first empty line;
            // at the end of the stream the line read is empty too. The Range
            // header, if any, says which bytes this request asks for.
            let mut range = None;
            let mut line = String::new();
            loop {
                line.clear();
                if let Err(error) = reader.read_line(&mut line) {
                    panic!("the cutting listener could not read the request head: {error}");
                }
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("range") {
                        range = Some(value.trim().to_owned());
                    }
                }
            }
            let asked = Asked {
                line: request_line.trim().to_owned(),
                range,
            };
            kept.lock()
                .expect("the request log is not poisoned")
                .push(asked);
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

/// Objects are read in resumable ranges, so a body cut after some of its
/// bytes made progress and the GET asks once more, for the rest from the
/// first byte it lacks; a dependency that answers that ask with the whole
/// object again instead of the range ends the GET naming the range it would
/// not serve, and nothing is asked a third time.
#[test]
fn a_body_cut_short_is_asked_once_for_the_rest_and_a_refused_range_ends_the_get() {
    let run = Run::new("cut");
    let (address, requests) = cutting_listener(run.free_port());
    let output = run.storage_get(&address);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a body cut short is a failed GET: {said}"
    );
    let cut = first_word().len();
    assert!(
        said.contains(&format!("asking for the rest from byte {cut}")),
        "the cut is said with the byte the rest is asked from: {said}"
    );
    assert!(
        said.contains(&format!("refused the byte range beginning at {cut}")),
        "the dependency's refusal of the range is the cause: {said}"
    );
    let seen = requests.lock().expect("the request log is not poisoned");
    match seen.as_slice() {
        [first, rest] => {
            assert!(
                first.line.starts_with("GET /api/object") && first.range.is_none(),
                "the first request is the object GET, whole: {first:?}"
            );
            assert!(
                rest.line.starts_with("GET /api/object"),
                "the second request is the object GET: {rest:?}"
            );
            let from_cut = format!("bytes={cut}-");
            assert!(
                rest.range.as_deref().is_some_and(|range| range.starts_with(&from_cut)),
                "the second request asks for the rest from the byte the cut reached: {rest:?}"
            );
        }
        asked => panic!(
            "the dependency is asked for the object and once for the rest, never a third time: \
             {asked:?}"
        ),
    }
}

/// `stado host ping` opens its beacon store before it resolves or reaches
/// the host, so a store nobody listens for ends it with that refusal.
#[test]
fn a_refused_object_api_ends_host_ping_at_once() {
    let run = Run::new("ping");
    let address = format!("{}:{}", Ipv4Addr::LOCALHOST, run.free_port());
    let output = run.host_ping(&address);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(!output.status.success(), "a refused store ended the ping well: {said}");
    let refusal = format!("stado host ping {HOST}: the beacon store did not open");
    assert!(said.contains(&refusal), "the refusal names the command: {said}");
    let cause = said.to_lowercase().contains("connection refused");
    assert!(cause, "the refusal names its cause: {said}");
    // The waits on the object API, by the event their line opens with.
    let place = format!("object API http://{address}");
    let waits = |event: &'static str| {
        said.lines()
            .filter(|line| line.starts_with(event) && line.contains("/api/object"))
            .collect::<Vec<_>>()
    };
    match (waits("czekam: ").as_slice(), waits("blad czekania: ").as_slice()) {
        ([asked], [failed]) => assert!(
            asked.contains(LAYOUT) && asked.contains(&place) && failed.contains(&place),
            "the one request is the store's layout marker at {place}: {asked} / {failed}"
        ),
        (asked, failed) => panic!(
            "the store is asked once and its refusal ends the ping: {asked:?} / {failed:?}"
        ),
    }
    let ssh = said.lines().any(|line| line.starts_with("czekam: ") && line.contains("ssh"));
    assert!(!ssh, "an SSH step ran after the store refused: {said}");
}

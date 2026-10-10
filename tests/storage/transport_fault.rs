//! An object upload whose peer ends the connection mid-body is refused as the
//! transport's failure, worth retrying, naming the chunk it was sending; it
//! is not the caller's "request or credentials" with no code.
//!
//! The real `stado storage put` is pointed at a loopback listener this test
//! owns, which accepts each connection, reads the request line only and
//! drops the socket while the body is still arriving. Unread bytes in the
//! kernel's receive buffer make that drop a reset (`connection reset by
//! peer` or `broken pipe` on the sending side), which is exactly what a
//! publisher saw from the object API after many chunks of a large archive.
//! The object is larger than one chunk, so the upload takes the chunked
//! path and the refusal carries the chunk's coordinates.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use stado::primitives::failure::FailureCode;
use stado::remote::object_store::OBJECT_API_CHUNK_BYTES;

const URI: &str = "stado://probe/transport/reset.bin";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("storage-transport-fault");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("create the run directory");
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let output = self
            .stado()
            .args(["host", "free-port-local"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        }
    }

    /// A loopback listener that resets every connection after its request
    /// line; answers the address the real stado is pointed at.
    fn resetting_listener(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut request_line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut request_line);
                // Dropped here with the body unread: the kernel answers the
                // sender with a reset, not a clean close.
                drop(stream);
            }
        });
        address
    }

    /// An object larger than one upload chunk, so the put is chunked.
    fn source(&self) -> PathBuf {
        let path = self.root.join("source.bin");
        let bytes = vec![u8::MAX; OBJECT_API_CHUNK_BYTES.saturating_add(OBJECT_API_CHUNK_BYTES)];
        std::fs::write(&path, bytes).expect("write the object to upload");
        path
    }

    fn put(&self, api: &str, source: &Path) -> Output {
        let output = self
            .stado()
            .args(["storage", "put", URI])
            .arg(source)
            .env("STADO_API_URL", format!("http://{api}"))
            .env("STADO_API_TOKEN", "transport-fault-probe")
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("put.stderr"), &output.stderr)
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
fn an_upload_the_peer_resets_is_the_transports_failure_naming_the_chunk() {
    let run = Run::new();
    let api = run.resetting_listener();
    let source = run.source();
    let output = run.put(&api, &source);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(FailureCode::RETRY_EXIT),
        "a transfer the network ended exits with the fleet's retry status: {said}"
    );
    let code = format!("[{}]", FailureCode::InfraDown.as_str());
    assert!(
        said.contains(&code) && said.contains("our failure"),
        "the operator line states the transport's own code, not the caller's request: {said}"
    );
    assert!(
        !said.contains(FailureCode::Unknown.as_str()),
        "a reset is never reported without a code: {said}"
    );
    assert!(
        said.contains("object chunk PUT ") && said.contains(" for upload ") && said.contains(URI),
        "the refusal names the chunk, the upload and the object it was sending: {said}"
    );
    assert!(
        said.contains("reset") || said.contains("broken pipe") || said.contains("closed"),
        "the operating system's own cause is kept in the sentence: {said}"
    );
}

//! A short object body is a transport failure, not a malformed object.
//!
//! `providers::local::disk_cleanup::fetch_canonical_registry` reads the
//! canonical registry through `RegistryStore::read_versioned` and hands the
//! body straight to `serde_json::from_str`. The Stado object route answers
//! every read with a `Content-Length` (41,041 bytes for the live registry) and
//! `Accept-Ranges: bytes`, and the backend used to return `response.bytes()`
//! without comparing the two — so a body that stopped short arrived as a
//! document that does not parse, which the janitor journals as `ValueError`.
//! The same shape on the sibling `/api/release/object` route is what the open
//! PR #317 is for; that route's readers live in `cli/storage.rs` and
//! `deploy/host_release.rs` and are not touched here.
//!
//! What is defended, against a stand-in object gateway on loopback:
//!
//! * a body shorter than the `Content-Length` it declares fails, in the
//!   `StorageError` family (so `disk_cleanup` classes it `OSError`), naming
//!   both byte counts;
//! * the same holds on the versioned read the janitor actually uses, which
//!   reads its own body and so needed the same shared reader;
//! * a response with no declared length is unchanged — it succeeds, because
//!   there is nothing for it to disagree with;
//! * a body that matches its declared length succeeds and parses.
//!
//! Isolation: the gateway is a `TcpListener` on 127.0.0.1:0 inside this
//! process and the bearer is a throwaway file in a `TempDir`. Nothing here
//! reaches the operator's real gateway, registry or vault.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;

use stado::queue::{BlobBackend, StadoObjectBackend, StorageError};

/// The namespace the live control plane resolves, and one `ObjectRef` accepts.
const NAMESPACE: &str = "probierz";

/// The registry blob, spelled as `targets::REGISTRY_BLOB` spells it.
const KEY: &str = "registry.json";

/// The version token the versioned route must carry, so that read fails on the
/// body and not on a missing header.
const VERSION: &str = "5dbf12dc1d9c8797002421829f610a72a0d8d76b3be11fe522246196c795b6a8";

/// How the gateway answers one read.
#[derive(Clone, Copy)]
enum Answer {
    /// Declare `declared` bytes, send `body`, close.
    Declared { declared: usize },
    /// Declare `declared` bytes and then frame the body with
    /// `Transfer-Encoding: chunked`, terminated properly.
    DeclaredChunked { declared: usize },
    /// Send the body with no `Content-Length` at all, then close: the length is
    /// the connection close, which is what a chunked or streaming route does.
    Unlengthed,
}

/// Read one HTTP request head off `stream` and discard it.
fn read_head(stream: &mut TcpStream) -> bool {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => return false,
            Ok(_) => raw.push(byte[0]),
            Err(_) => return false,
        }
    }
    true
}

/// A stand-in object gateway that answers exactly one read the given way.
///
/// Returns the origin to configure the backend with. The listener thread ends
/// after one connection; every test here performs one read.
fn gateway(answer: Answer, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port is bindable");
    let origin = format!("http://{}", listener.local_addr().expect("bound address"));
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        if !read_head(&mut stream) {
            return;
        }
        let head = match answer {
            Answer::Declared { declared } => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {declared}\r\nAccept-Ranges: bytes\r\n\
                 X-Stado-Version: {VERSION}\r\nConnection: close\r\n\r\n"
            ),
            Answer::DeclaredChunked { declared } => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {declared}\r\nTransfer-Encoding: chunked\r\n\
                 Accept-Ranges: bytes\r\nX-Stado-Version: {VERSION}\r\n\
                 Connection: close\r\n\r\n"
            ),
            Answer::Unlengthed => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Accept-Ranges: bytes\r\nX-Stado-Version: {VERSION}\r\n\
                 Connection: close\r\n\r\n"
            ),
        };
        let _ = stream.write_all(head.as_bytes());
        if matches!(answer, Answer::DeclaredChunked { .. }) {
            let _ = stream.write_all(format!("{:x}\r\n", body.len()).as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.write_all(b"\r\n0\r\n\r\n");
        } else {
            let _ = stream.write_all(&body);
        }
        let _ = stream.flush();
        // Closing here is what makes a short declared body observable: the
        // gateway says it is finished while the count disagrees.
        drop(stream);
    });
    origin
}

/// One backend bound to `origin`, with a throwaway owner-only bearer.
fn backend(origin: &str) -> (StadoObjectBackend, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temp dir is creatable");
    let token = dir.path().join("bearer");
    std::fs::write(&token, "stand-in-bearer").expect("bearer file is writable");
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))
        .expect("bearer file takes owner-only mode");
    let backend = StadoObjectBackend::new(
        origin,
        NAMESPACE,
        token.to_str().expect("temp path is UTF-8"),
        "",
    )
    .expect("loopback origin with an owner-only bearer is a valid backend");
    (backend, dir)
}

/// The live registry document's size, so the numbers in the failure are the
/// ones an operator reading this journal would see.
const REGISTRY_BYTES: usize = 41_041;

/// The early-close case, recorded because it is the one this reader does NOT
/// have to catch: on an HTTP/1.1 response whose `Content-Length` is declared
/// and whose socket then closes early, hyper refuses the body itself and the
/// backend already reports `StorageError::Http`. That is the transport class
/// too, so the guard below is not what saves this shape — this test exists so
/// nobody attributes it to the new reader, and so a future client change that
/// stops enforcing framing is visible here.
#[tokio::test]
async fn an_early_close_on_a_declared_length_is_refused_by_the_client_itself() {
    let origin = gateway(
        Answer::Declared {
            declared: REGISTRY_BYTES,
        },
        vec![b'{'; 128],
    );
    let (backend, _dir) = backend(&origin);
    let error = backend
        .download_text(KEY)
        .await
        .expect_err("a body 128 bytes into a declared 41041 is not the object");
    assert!(
        matches!(error, StorageError::Http(_)),
        "hyper's own framing check owns this case; got {error:?}"
    );
}

/// The case the new reader owns: framing that ends cleanly at a size the
/// response's own `Content-Length` contradicts. A body chunked to its end is
/// complete as far as the client is concerned, so `bytes()` returns `Ok` and
/// only a comparison against the declaration can tell that 128 bytes are not
/// the 41,041-byte object.
#[tokio::test]
async fn plain_read_refuses_a_body_shorter_than_its_declared_length() {
    let origin = gateway(
        Answer::DeclaredChunked {
            declared: REGISTRY_BYTES,
        },
        vec![b'{'; 128],
    );
    let (backend, _dir) = backend(&origin);
    let error = backend
        .download_text(KEY)
        .await
        .expect_err("a body 128 bytes into a declared 41041 is not the object");
    assert!(
        matches!(error, StorageError::Other(_)),
        "a short transfer belongs to the transport family disk_cleanup maps to \
         OSError, not to a parse class; got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("128") && message.contains("41041"),
        "the failure must say how short it was; got {message:?}"
    );
    assert!(
        message.contains(KEY),
        "the failure must name the object; got {message:?}"
    );
}

#[tokio::test]
async fn versioned_read_refuses_a_body_shorter_than_its_declared_length() {
    let origin = gateway(
        Answer::DeclaredChunked {
            declared: REGISTRY_BYTES,
        },
        vec![b'{'; 128],
    );
    let (backend, _dir) = backend(&origin);
    let error = backend
        .download_text_versioned(KEY)
        .await
        .expect_err("the read the janitor uses must refuse a short body too");
    assert!(matches!(error, StorageError::Other(_)), "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("128") && message.contains("41041"),
        "the failure must say how short it was; got {message:?}"
    );
}

#[tokio::test]
async fn a_response_with_no_declared_length_is_unchanged() {
    let document = br#"{"schema_version": 2}"#.to_vec();
    let origin = gateway(Answer::Unlengthed, document.clone());
    let (backend, _dir) = backend(&origin);
    let text = backend
        .download_text_versioned(KEY)
        .await
        .expect("an unlengthed response has nothing to disagree with")
        .expect("the gateway answered 200, so the object exists");
    assert_eq!(text.content.as_bytes(), document.as_slice());
    assert_eq!(text.version, VERSION);
}

#[tokio::test]
async fn a_body_matching_its_declared_length_still_parses() {
    let document = br#"{"schema_version": 2}"#.to_vec();
    let origin = gateway(
        Answer::Declared {
            declared: document.len(),
        },
        document.clone(),
    );
    let (backend, _dir) = backend(&origin);
    let text = backend
        .download_text_versioned(KEY)
        .await
        .expect("a whole body is accepted")
        .expect("the gateway answered 200, so the object exists");
    let value: serde_json::Value =
        serde_json::from_str(&text.content).expect("a whole registry document parses");
    assert_eq!(value["schema_version"], 2);
}

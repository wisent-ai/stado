//! The one dashboard every case shares, the store it is rooted in, and the
//! raw HTTP the cases speak to it.
//!
//! The dashboard is started once for the whole area: its store root and its
//! Skarbiec endpoints come from process environment the product reads once, so
//! a second dashboard with different settings could not exist in this process
//! anyway. Each case owns its own object key inside the shared store.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::vault;

/// The namespace these cases write in. One of the product's own active object
/// namespaces, so its configuration parser accepts the declaration.
pub const NAMESPACE: &str = "probierz";

/// The root every qualified object path starts with: `ObjectRef::storage_path`
/// builds `ecosystem/<namespace>/<key>`, and a filesystem backend stores the
/// name it is handed.
pub const STORE_PREFIX: &str = "ecosystem";

pub struct Harness {
    root: tempfile::TempDir,
    addr: SocketAddr,
}

impl Harness {
    /// The file the store keeps `key` in, so a case can write it, read it, or
    /// take away the permission to reach it.
    pub fn object_file(&self, key: &str) -> PathBuf {
        self.root
            .path()
            .join(STORE_PREFIX)
            .join(NAMESPACE)
            .join(key)
    }

    /// Where the filesystem backend keeps one object's metadata sidecar, which
    /// is the state a successful metadata write has to leave behind.
    pub fn metadata_file(&self, key: &str) -> PathBuf {
        self.root
            .path()
            .join(".metadata")
            .join(STORE_PREFIX)
            .join(NAMESPACE)
            .join(key)
    }

    /// Put `content` in the store under `key`, creating the directories the
    /// store would have created itself.
    pub fn write_object(&self, key: &str, content: &str) -> PathBuf {
        let path = self.object_file(key);
        std::fs::create_dir_all(path.parent().expect("an object has a directory"))
            .expect("the object's directory is creatable");
        std::fs::write(&path, content).expect("the object is writable");
        path
    }

    pub fn put(&self, target: &str, bearer: &str, body: &str) -> Answer {
        let mut stream = self.connect();
        let request = format!(
            "PUT {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .expect("the dashboard accepts the request");
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .expect("the dashboard answers and closes");
        Answer::parse(&String::from_utf8_lossy(&raw))
    }

    /// Connect, retrying while the listener is still coming up. The dashboard
    /// accepts only after its startup validation has recorded every boundary
    /// verdict, so the first connection of the area waits for that.
    fn connect(&self) -> TcpStream {
        let mut refusals = usize::MIN;
        loop {
            match TcpStream::connect(self.addr) {
                Ok(stream) => return stream,
                Err(error) => {
                    refusals += usize::from(true);
                    assert!(
                        refusals < ATTEMPTS,
                        "the dashboard never accepted a loopback connection \
                         in {refusals} attempts: {error}"
                    );
                    std::thread::yield_now();
                }
            }
        }
    }
}

/// Connection attempts before the dashboard is declared not to be coming up.
/// A count rather than a clock: the answer does not depend on how fast this
/// machine is, only on the listener eventually accepting.
const ATTEMPTS: usize = 20_000;

/// One HTTP answer, reduced to what these contracts are written against.
pub struct Answer {
    pub status: u16,
    pub body: String,
}

impl Answer {
    fn parse(raw: &str) -> Self {
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .expect("the answer has a head and a body");
        let status = head
            .split_whitespace()
            .nth(usize::from(true))
            .and_then(|status| status.parse::<u16>().ok())
            .expect("the status line carries a code");
        Self {
            status,
            body: body.to_string(),
        }
    }
}

/// A directory the owner may not traverse, restored when the guard is dropped.
///
/// This is how this operating system refuses a read: without the execute bit
/// the kernel answers `EACCES` for anything below the directory, even to its
/// owner, so the store can neither confirm nor deny the object inside it. The
/// mode is put back in `Drop`, including when a case panics, so nothing else
/// in the process trips over a store it cannot read.
pub struct Unreadable {
    directory: PathBuf,
    restore: u32,
}

impl Unreadable {
    pub fn close(directory: &Path) -> Self {
        let restore = std::fs::metadata(directory)
            .expect("the directory exists before it is closed")
            .permissions()
            .mode();
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o000))
            .expect("the directory takes the closed mode");
        Self {
            directory: directory.to_path_buf(),
            restore,
        }
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(
            &self.directory,
            std::fs::Permissions::from_mode(self.restore),
        );
    }
}

/// The dashboard, started once for the whole area.
static HARNESS: LazyLock<Harness> = LazyLock::new(start);

pub fn harness() -> &'static Harness {
    &HARNESS
}

fn start() -> Harness {
    let root = tempfile::TempDir::new().expect("a store root of this area's own");
    let coordinator_grant = root.path().join("coordinator-grant");
    let object_grant = root.path().join("object-verifier-grant");
    vault::write_grant(&coordinator_grant, "coordinator-grant-value");
    vault::write_grant(&object_grant, "object-verifier-grant-value");
    let broker = vault::spawn();

    std::env::set_var("HOME", root.path());
    std::env::set_var("WC_STORAGE_BACKEND", "local");
    std::env::set_var("WC_LOCAL_STORAGE_PATH", root.path());
    // A set-but-missing STADO_CONFIG disables config-file discovery, so the
    // developer's own configuration cannot decide what this area measures.
    std::env::set_var("STADO_CONFIG", root.path().join("no-such-config.json"));
    for variable in [
        "COMPUTE_API_KEY",
        "COMPUTE_API_URL",
        "WC_PROFILES_DIR",
        "WC_BACKUP_STORAGE_BACKEND",
    ] {
        std::env::remove_var(variable);
    }
    // Every boundary but `object` is pointed at a dead loopback port: they must
    // fail, fail at once, and never reach a real vault.
    for variable in [
        "WC_SKARBIEC_URL",
        "WC_RELEASE_SKARBIEC_URL",
        "WC_MACHINE_SKARBIEC_URL",
        "WC_SERVICE_SKARBIEC_URL",
        "WC_RATE_LIMIT_SKARBIEC_URL",
        "WC_INTEGRATION_SKARBIEC_URL",
        "WC_INTEGRATION_PROVIDER_SKARBIEC_URL",
    ] {
        std::env::set_var(variable, "http://127.0.0.1:1");
    }
    std::env::set_var("WC_SKARBIEC_TOKEN_FILE", &coordinator_grant);
    std::env::set_var("WC_OBJECT_SKARBIEC_URL", format!("http://{broker}"));
    std::env::set_var("WC_OBJECT_SKARBIEC_TOKEN_FILE", &object_grant);
    std::env::set_var("WC_OBJECT_API_NAMESPACES", vault::namespaces_document());
    std::env::set_var("WC_DASHBOARD_BOUNDARY_ATTEMPTS", "1");

    // The dashboard binds a fixed port, so the port is chosen here and
    // released; the listener claims it immediately.
    let addr = {
        let probe = TcpListener::bind(("127.0.0.1", 0)).expect("a free loopback port exists");
        probe.local_addr().expect("the probe has an address")
    };
    let port = i64::from(addr.port());
    std::thread::Builder::new()
        .name("outage-dashboard".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("dashboard runtime");
            // Exactly what `stado dashboard --bind 127.0.0.1 --port <port>` runs.
            if let Err(error) = runtime.block_on(stado::dashboard::serve(
                Some("127.0.0.1"),
                Some(port),
                false,
            )) {
                eprintln!("[test] dashboard exited: {error}");
            }
        })
        .expect("dashboard thread starts");
    Harness { root, addr }
}

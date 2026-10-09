//! A real Stado command that waits on a lock another process holds says so
//! on stderr before it blocks, and how long it waited once the lock is let
//! go.
//!
//! The command is `stado registry host show` against an isolated local
//! store under this checkout's build directory (`WC_STORAGE_BACKEND=local`,
//! `WC_LOCAL_STORAGE_PATH`, a `STADO_CONFIG` that names no file). The test
//! holds the store's real kernel lock on `registry.json` — the same
//! `.locks/<sha256 of the path>` file every Stado process locks — reads the
//! command's stderr line by line until the `czekam` line names that lock,
//! checks the command is still running behind it, lets the lock go and reads
//! the `koniec czekania` line with the same id and the command's own answer.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use fs2::FileExt;
use sha2::{Digest, Sha256};

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("wait-held-lock");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        Self { root }
    }

    fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// The kernel lock file Stado takes for `path` in this store.
    fn lock_of(&self, path: &str) -> PathBuf {
        self.store()
            .join(".locks")
            .join(hex::encode(Sha256::digest(path.as_bytes())))
    }

    fn spawn(&self, args: &[&str]) -> Child {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .current_dir(&self.root)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real stado")
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The value of one `name: value` field of a wait line.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split("; ")
        .find_map(|part| part.strip_prefix(&format!("{name}: ")))
}

#[test]
fn a_registry_read_behind_a_held_store_lock_names_the_lock_before_it_blocks() {
    let store = Store::new("registry");
    let lock_path = store.lock_of("registry.json");
    let held: File = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(&lock_path)
        .expect("open the store's registry lock");
    held.lock_exclusive().expect("hold the store's registry lock");

    let mut child = store.spawn(&["registry", "host", "show", "example-host", "--path", "kind"]);
    let mut stderr = BufReader::new(child.stderr.take().expect("stado's stderr"));
    let mut said = Vec::new();
    let started = loop {
        let mut line = String::new();
        let read = stderr.read_line(&mut line).expect("read stado's stderr");
        assert!(read > 0, "stado ended before saying it waits: {said:?}");
        said.push(line.clone());
        if line.starts_with("czekam: exclusive lock on the local store's registry.json") {
            break line;
        }
    };
    assert_eq!(field(&started, "gdzie"), Some(lock_path.display().to_string().as_str()), "{started}");
    assert_eq!(field(&started, "rodzaj"), Some("blokada"), "{started}");
    let id = field(&started, "id").expect("the wait line carries its id").trim().to_string();
    assert!(field(&started, "od").is_some(), "{started}");
    assert!(
        child.try_wait().expect("ask whether stado ended").is_none(),
        "stado did not wait behind the held lock"
    );

    held.unlock().expect("let the registry lock go");
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).expect("read the rest of stado's stderr");
    let status = child.wait().expect("stado ends");
    let ended = rest
        .lines()
        .find(|line| line.starts_with("koniec czekania: exclusive lock on the local store's registry.json"))
        .expect("stado says the lock wait ended once the lock is let go");
    assert_eq!(field(ended, "id").map(str::trim), Some(id.as_str()), "{ended}\n{rest}");
    assert!(field(ended, "trwalo").is_some_and(|took| took.trim().ends_with('s')), "{ended}");
    assert!(!status.success(), "an empty store has no registry to show");
    assert!(rest.contains("no registry document at local:registry.json"), "{rest}");
}

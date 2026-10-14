//! A wait whose process was killed is ended by the next reading of the
//! register, not listed as open for good.
//!
//! The real `stado registry host show` runs against an isolated local store
//! under this checkout's build directory whose registry lock the test holds,
//! so the command opens a wait on that lock and blocks. `stado waits --json`
//! lists the wait as open under the command's pid. The command is then
//! killed, which leaves its file in the register; the next `stado waits`
//! reports the wait ended, with the line it held, and removes the file, and
//! the reading after that holds it no more. The refusals: a process with
//! neither `STADO_WAITS_DIR` nor `HOME` has no register, and a register file
//! not named `<pid>-<n>` is named instead of being skipped.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use fs2::FileExt;
use serde_json::Value;
use sha2::{Digest, Sha256};

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("wait-ended-process");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        Self { root }
    }

    /// The kernel lock file Stado takes for `path` in this store.
    fn lock_of(&self, path: &str) -> PathBuf {
        self.root
            .join("store/.locks")
            .join(hex::encode(Sha256::digest(path.as_bytes())))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("STADO_WAITS_DIR", self.waits())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn spawn(&self, args: &[&str]) -> Child {
        self.command(args).spawn().expect("start the real stado")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run the real stado")
    }

    /// `stado waits --json`, which must answer.
    fn listing(&self) -> Value {
        let output = self.run(&["waits", "--json"]);
        assert!(
            output.status.success(),
            "stado waits --json: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("stado waits --json prints JSON")
    }

    /// The isolated register of open waits.
    fn waits(&self) -> PathBuf {
        self.root.join("waits")
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The wait `id` a listing holds under `section`, if it holds it.
fn find<'a>(listing: &'a Value, section: &str, id: &str) -> Option<&'a Value> {
    let Some(waits) = listing[section].as_array() else {
        panic!("the listing has no {section} array: {listing}");
    };
    waits.iter().find(|wait| wait["id"] == id)
}

/// The value of one `name: value` field of a wait line.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split("; ")
        .find_map(|part| part.strip_prefix(&format!("{name}: ")))
}

#[test]
fn a_wait_whose_process_was_killed_is_ended_by_the_next_reading() {
    let store = Store::new("killed");
    let held: File = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(store.lock_of("registry.json"))
        .expect("open the store's registry lock");
    held.lock_exclusive()
        .expect("hold the store's registry lock");

    let mut child = store.spawn(&["registry", "host", "show", "example-host", "--path", "kind"]);
    let mut stderr = BufReader::new(child.stderr.take().expect("stado's stderr")).lines();
    let mut said = Vec::new();
    let started = loop {
        let Some(line) = stderr.next() else {
            panic!("stado ended before saying it waits: {said:?}");
        };
        let line = line.expect("read stado's stderr");
        said.push(line.clone());
        if line.starts_with("czekam: exclusive lock on the local store's registry.json") {
            break line;
        }
    };
    let id = field(&started, "id")
        .expect("the wait line carries its id")
        .to_string();

    let standing = store.listing();
    let Some(listed) = find(&standing, "open", &id) else {
        panic!("the standing wait {id} is listed open: {standing}");
    };
    assert_eq!(listed["pid"], child.id(), "{standing}");
    assert_eq!(listed["line"], started.as_str(), "{standing}");
    assert!(find(&standing, "ended", &id).is_none(), "{standing}");

    child.kill().expect("kill the waiting stado");
    child.wait().expect("the killed stado is reaped");
    let file = store.waits().join(&id);
    assert!(
        file.exists(),
        "a killed process cannot remove its own wait, so its file is still there"
    );

    let after = store.listing();
    assert!(
        find(&after, "open", &id).is_none(),
        "a wait whose process is gone is not open: {after}"
    );
    let Some(ended) = find(&after, "ended", &id) else {
        panic!("the killed process's wait {id} is reported ended: {after}");
    };
    assert_eq!(
        ended["line"],
        started.as_str(),
        "the ended wait keeps the line it held when its process died: {after}"
    );
    assert!(
        !file.exists(),
        "the reading that ended the wait removed its file"
    );

    let next = store.listing();
    assert!(
        find(&next, "open", &id).is_none() && find(&next, "ended", &id).is_none(),
        "the next reading no longer holds the ended wait: {next}"
    );

    let text = store.run(&["waits"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let printed = String::from_utf8_lossy(&text.stdout);
    assert!(
        printed.starts_with(&format!("register {}: ", store.waits().display())),
        "the plain listing names the register it read: {printed}"
    );
    held.unlock().expect("let the registry lock go");
}

#[test]
fn a_register_that_cannot_be_read_is_refused_naming_why() {
    let store = Store::new("refused");

    let homeless = store
        .command(&["waits"])
        .env_remove("STADO_WAITS_DIR")
        .env_remove("HOME")
        .output()
        .expect("run the real stado");
    assert!(
        !homeless.status.success(),
        "a process with no register directory has nothing to list"
    );
    let said = String::from_utf8_lossy(&homeless.stderr);
    assert!(
        said.contains(
            "the register of open waits cannot be read: neither STADO_WAITS_DIR nor HOME is set, so the register of open waits has no directory"
        ),
        "{said}"
    );

    std::fs::create_dir_all(store.waits()).expect("create the register");
    let stray = store.waits().join("not-a-wait");
    std::fs::write(&stray, "czekam: nothing\n").expect("write a stray file into the register");
    let refused = store.run(&["waits", "--json"]);
    assert!(
        !refused.status.success(),
        "a stray file is not read as a wait"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains(&format!(
            "the register of open waits cannot be read: {} is not named <pid>-<n>, so it is no wait of this register; remove it",
            stray.display()
        )),
        "{said}"
    );
    assert!(stray.exists(), "a refused reading removes nothing");
}

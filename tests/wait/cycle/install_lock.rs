//! A lock wait that would close a cycle is refused at once, naming every
//! link; a lock wait with no cycle stands as it always did; `stado waits
//! cycles` names a cycle that stands.
//!
//! Every case runs the real `stado product install <product> --surface cli
//! --wait` in an isolated home, against the product's real surface lock
//! (`~/.stado/products/<product>/cli.lock`) that this test holds with
//! `flock` and whose holder record it writes as an installation does. The
//! processes that hold and wait are real: this test (the parent of every
//! install it starts, so its completion needs them), a `cat` that holds
//! nothing and waits on nothing, and the installs themselves.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Output, Stdio};

use serde_json::Value;

struct Run {
    root: PathBuf,
    held: Vec<File>,
    children: Vec<Child>,
}

impl Run {
    fn new(case: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/waits-cycle")
            .join(format!("{case}-{}", std::process::id()));
        fs::create_dir_all(root.join("home")).expect("create the isolated home");
        Self {
            root,
            held: Vec::new(),
            children: Vec::new(),
        }
    }

    fn stado(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("STADO_WAITS_DIR", self.root.join("waits"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// The surface lock of `product`'s CLI in the isolated home.
    fn lock(&self, product: &str) -> PathBuf {
        self.root
            .join("home/.stado/products")
            .join(product)
            .join("cli.lock")
    }

    /// Hold `lock` with `flock`, as an installation holds it, recording
    /// `pid` as its holder.
    fn hold(&mut self, lock: &Path, pid: u32) {
        fs::create_dir_all(lock.parent().expect("a lock has a directory"))
            .expect("create the product's state directory");
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock)
            .expect("open the surface lock");
        file.try_lock().expect("hold the surface lock");
        self.held.push(file);
        record(lock, pid);
    }

    /// A process that holds nothing and waits on nothing, until this test
    /// ends.
    fn bystander(&mut self) -> u32 {
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("start cat");
        let pid = child.id();
        self.children.push(child);
        pid
    }

    /// Start the install of `product` and read its stderr until its lock
    /// wait began; the install's pid.
    fn waiting_install(&mut self, product: &str) -> u32 {
        let mut child = self
            .stado(&["product", "install", product, "--surface", "cli", "--wait"])
            .spawn()
            .expect("start the real stado");
        let stderr = child.stderr.take().expect("the install's stderr is piped");
        let lock = self.lock(product);
        let said = until_waiting(stderr, &lock);
        let pid = child.id();
        self.children.push(child);
        assert!(
            said.is_some(),
            "the install of {product} never began its wait on {}",
            lock.display()
        );
        pid
    }

    fn install(&self, product: &str) -> Output {
        let output = self
            .stado(&["product", "install", product, "--surface", "cli", "--wait"])
            .output()
            .expect("run the real stado");
        fs::write(
            self.root.join(format!("install-{product}.stderr")),
            &output.stderr,
        )
        .expect("keep the install's stderr beside the run");
        output
    }

    fn cycles(&self) -> (Output, Value) {
        let output = self
            .stado(&["waits", "cycles", "--json"])
            .output()
            .expect("run the real stado");
        match serde_json::from_slice(&output.stdout) {
            Ok(listing) => (output, listing),
            Err(error) => panic!(
                "stado waits cycles --json printed no JSON ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.held.clear();
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Write the holder record of `lock` naming `pid`, as an installation does.
fn record(lock: &Path, pid: u32) {
    let record = serde_json::json!({
        "pid": pid,
        "command": "stado product install --surface cli",
        "acquired_at": chrono::Utc::now().to_rfc3339(),
        "phase": "placing",
        "step": null,
    });
    fs::write(lock, format!("{record}\n")).expect("write the holder record");
}

/// Read `stderr` until the line that begins the wait on `lock`; that line,
/// or none when the process ended first.
fn until_waiting(stderr: ChildStderr, lock: &Path) -> Option<String> {
    let place = lock.display().to_string();
    for line in BufReader::new(stderr).lines() {
        let line = line.expect("the install's standard error is readable");
        if line.starts_with("czekam:") && line.contains(&place) {
            return Some(line);
        }
    }
    None
}

#[test]
fn an_install_waiting_on_a_lock_its_own_parent_holds_is_refused_naming_the_cycle() {
    let mut run = Run::new("parent");
    let lock = run.lock("tama");
    run.hold(&lock, std::process::id());

    let output = run.install("tama");
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("czekanie odmowione"), "{said}");
    assert!(said.contains("a wait cycle"), "{said}");
    assert!(
        said.contains(&format!("held by process {}", std::process::id())),
        "{said}"
    );
    assert!(said.contains(&lock.display().to_string()), "{said}");
    assert!(
        !said
            .lines()
            .any(|line| line.starts_with("czekam:") && line.contains("rodzaj: blokada")),
        "the refused wait began: {said}"
    );
}

#[test]
fn a_lock_wait_without_a_cycle_stands_and_no_cycle_is_listed() {
    let mut run = Run::new("plain");
    let holder = run.bystander();
    let lock = run.lock("tama");
    run.hold(&lock, holder);

    run.waiting_install("tama");
    let (output, listing) = run.cycles();
    assert!(output.status.success(), "{listing}");
    assert_eq!(listing["cycles"], serde_json::json!([]), "{listing}");
}

#[test]
fn a_wait_closing_a_cycle_through_another_wait_is_refused_and_a_standing_cycle_is_listed() {
    let mut run = Run::new("chain");
    let bystander = run.bystander();
    let first = run.lock("tama");
    let second = run.lock("ster");
    run.hold(&first, bystander);
    let waiting = run.waiting_install("tama");
    run.hold(&second, waiting);

    // The second lock's holder waits on the first, which this test holds:
    // an install of the second, started by this test, could never end.
    record(&first, std::process::id());
    let output = run.install("ster");
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("czekanie odmowione"), "{said}");
    assert!(
        said.contains(&format!("held by process {waiting}")),
        "{said}"
    );
    assert!(
        said.contains(&format!("process {waiting}")) && said.contains(&first.display().to_string()),
        "the cycle does not name the link through the other wait: {said}"
    );

    // With the first lock's record on the bystander the second install has
    // no cycle and waits; once the first lock names it as holder, both
    // waits stand in a cycle that `stado waits cycles` names.
    record(&first, bystander);
    let closing = run.waiting_install("ster");
    record(&first, closing);
    let (output, listing) = run.cycles();
    assert!(!output.status.success(), "{listing}");
    let cycles = listing["cycles"].as_array().expect("cycles is a list");
    let [cycle] = cycles.as_slice() else {
        panic!("one cycle stands, not {}: {listing}", cycles.len());
    };
    let mut pids: Vec<u64> = cycle["links"]
        .as_array()
        .expect("links is a list")
        .iter()
        .filter_map(|link| link["pid"].as_u64())
        .collect();
    pids.sort_unstable();
    let mut expected = vec![u64::from(waiting), u64::from(closing)];
    expected.sort_unstable();
    assert_eq!(pids, expected, "{listing}");
}

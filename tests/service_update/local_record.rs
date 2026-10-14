//! `stado service update --from-archive` installs a locally built archive
//! from the registry this machine already holds, so a host whose registry
//! authority's snapshot stands unanswered can still be repaired.
//!
//! The real `stado` runs with an isolated `HOME` under the checkout's build
//! directory and a last-known-good registry copy that names this machine as a
//! host managing one launchd unit and, as the registry authority, a loopback
//! listener this test owns that accepts every connection and sends nothing:
//! a snapshot asked of it would never come. The update installs a real
//! archive into the isolated `HOME`, repoints the unit file onto `current`,
//! and the authority must never have been connected to.
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

const SERVICE: &str = "archive-example";
const LABEL: &str = "com.example.archive-example";
const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";

struct Run {
    root: PathBuf,
    /// Whether the silent authority accepted any connection; it must not,
    /// because nobody may ask it for a snapshot.
    asked: Arc<AtomicBool>,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("service-update-local-record");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        for directory in [
            ".stado/cache",
            ".stado/bin",
            "LaunchAgents",
            "tmp",
            "bundle",
        ] {
            std::fs::create_dir_all(root.join(directory)).expect("create the run directory");
        }
        Self {
            root,
            asked: Default::default(),
        }
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
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let output = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The authority's SSH destination: a listener that accepts, keeps every
    /// connection open and never speaks, so a snapshot asked of it would
    /// stand forever.
    fn silent_authority(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        let asked = Arc::clone(&self.asked);
        std::thread::spawn(move || {
            let mut open = Vec::new();
            for stream in listener.incoming().flatten() {
                asked.store(true, Ordering::SeqCst);
                open.push(stream);
            }
        });
        address
    }

    fn program(&self) -> PathBuf {
        self.root.join(".stado/bin").join(SERVICE)
    }

    fn unit_path(&self) -> PathBuf {
        self.root
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    /// The installed unit, running the program from `~/.stado/bin`.
    fn write_unit(&self) {
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\
             <key>Label</key><string>{LABEL}</string>\
             <key>ProgramArguments</key><array><string>{}</string><string>serve</string></array>\
             </dict></plist>\n",
            self.program().display()
        );
        std::fs::write(self.unit_path(), plist).expect("write the unit file");
    }

    /// The copy this stado resolves everything from: this machine as the
    /// host that manages the unit, the silent listener as the authority.
    /// Answers the name of this machine's target.
    fn write_last_good(&self, authority_ssh: &str) -> String {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets
            .first_mut()
            .expect("the template declares the reader first");
        let name = reader["name"].as_str().unwrap().to_string();
        reader["hostnames"] = json!([hostname()]);
        reader["services"] = json!([{
            "name": SERVICE,
            "label": LABEL,
            "kind": "launchd",
            "path": self.unit_path(),
            "program": self.program(),
            "args": ["serve"],
        }]);
        let authority = targets
            .last_mut()
            .expect("the template declares the authority last");
        authority["name"] = json!(AUTHORITY);
        authority["ssh"] = json!(authority_ssh);
        authority["services"] = json!([]);
        document["service_directory"]["authority"] =
            json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
        document["service_directory"]["services"] = json!({});
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({
                "read_at": "1970-01-01T00:00:00Z",
                "generation": "example",
            }))
            .unwrap(),
        )
        .unwrap();
        name
    }

    /// A release archive carrying the service's executable at its root, as a
    /// local build packs it.
    fn write_archive(&self) -> PathBuf {
        let executable = self.root.join("bundle").join(SERVICE);
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let made_executable = Command::new("chmod")
            .arg("+x")
            .arg(&executable)
            .status()
            .expect("run chmod");
        assert!(
            made_executable.success(),
            "chmod made the program executable"
        );
        let archive = self.root.join(format!("{SERVICE}.tar.gz"));
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(self.root.join("bundle"))
            .arg(SERVICE)
            .status()
            .expect("run tar");
        assert!(packed.success(), "tar packed the archive");
        archive
    }

    fn update(&self, host: &str, archive: &Path) -> Output {
        let output = self
            .stado()
            .args([
                "service",
                "update",
                SERVICE,
                "--host",
                host,
                "--from-archive",
            ])
            .arg(archive)
            .arg("--json")
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("update.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        std::fs::write(self.root.join("update.stdout"), &output.stdout)
            .expect("keep the command's stdout beside the run");
        output
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

#[test]
fn an_archive_installs_from_this_machines_record_without_asking_the_authority() {
    let run = Run::new();
    let authority_ssh = run.silent_authority();
    let host = run.write_last_good(&authority_ssh);
    run.write_unit();
    let archive = run.write_archive();

    let output = run.update(&host, &archive);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        output.status.success(),
        "the archive installs from this machine's record: {said}{printed}"
    );
    assert!(
        said.contains("without asking the registry authority"),
        "the command says it acts from this machine's record: {said}"
    );
    assert!(
        !run.asked.load(Ordering::SeqCst),
        "the registry authority was never connected to: {said}"
    );
    let report: Value = serde_json::from_str(printed.trim()).expect("the update prints JSON");
    assert_eq!(report["status"], "updated", "{report}");
    let installed = run
        .root
        .join(".stado/services")
        .join(SERVICE)
        .join("current/darwin-arm")
        .join(SERVICE);
    let ran = Command::new(&installed)
        .status()
        .expect("the installed program can be started");
    assert!(ran.success(), "the installed program runs");
    let unit = std::fs::read_to_string(run.unit_path()).unwrap();
    assert!(
        unit.contains(&installed.display().to_string()),
        "the unit now runs the installed version through current: {unit}"
    );

    std::fs::write(
        run.root.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "authority_ssh": authority_ssh,
            "host": host,
            "exit_status": output.status.to_string(),
            "stderr": said,
            "stdout": printed,
            "installed": installed,
        }))
        .unwrap(),
    )
    .unwrap();
}

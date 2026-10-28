//! The isolated run `main.rs` drives: the real stado with `HOME` under the
//! test's output directory, a last-known-good registry copy naming this
//! machine as the reader, and a registry authority served by a real OpenSSH
//! server that runs the real stado, answers one read and is then stopped, so
//! it accepts every later connection and sends nothing (`hung_ssh`).
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

use crate::hung_ssh::HungSsh;

pub(crate) const AUTHORITY: &str = "other-host";
pub(crate) const ABANDONED: &str = "request abandoned unanswered";
/// What the reader says when the authority answered its read with its own
/// refusal: the live authority before it is stopped.
const AUTHORITY_ANSWERED: &str = "registry authority exited with";

pub(crate) struct Run {
    pub(crate) root: PathBuf,
    authority: Option<HungSsh>,
}

impl Run {
    /// A run of its own, named so two journeys of one test binary never
    /// share a home.
    pub(crate) fn new(name: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/host-silence-silent-authority");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self {
            root,
            authority: None,
        }
    }

    /// The real stado with nothing of this account's configuration.
    pub(crate) fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
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

    /// Serve the registry authority, let it answer one registry read, then
    /// stop it. Answers its SSH destination and the refresh interval the
    /// reader declares, the one measure of the wait.
    pub(crate) fn hung_authority(&mut self) -> (String, i64) {
        self.hung_authority_declared_on(Declarer::Reader)
    }

    /// [`Run::hung_authority`] with the resolver declaration, and with it
    /// the refresh interval answered, on `declarer` alone.
    pub(crate) fn hung_authority_declared_on(&mut self, declarer: Declarer) -> (String, i64) {
        let authority = HungSsh::serve(&self.root, self.free_port());
        let destination = authority.destination.clone();
        let refresh_seconds = self.write_last_good(&destination, declarer);
        self.authority = Some(authority);
        let answered = self.registry_self("answered");
        let said = String::from_utf8_lossy(&answered.stderr).into_owned();
        assert!(
            said.contains(AUTHORITY_ANSWERED),
            "the live authority answers the read before it is stopped: {said}"
        );
        if let Some(authority) = &self.authority {
            authority.hang();
        }
        (destination, refresh_seconds)
    }

    /// The copy this stado resolves its authority through: the shared
    /// directory template with this machine as the reader, the served
    /// authority's SSH destination, and the real stado as the command the
    /// authority runs. The template's resolver declaration stands on
    /// `declarer` alone. Answers the refresh interval it declares.
    fn write_last_good(&self, authority_ssh: &str, declarer: Declarer) -> i64 {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets
            .first_mut()
            .expect("the template declares the reader first");
        let declaration = reader["service_resolver"].clone();
        let refresh_seconds = declaration["refresh_seconds"]
            .as_i64()
            .expect("the template declares the reader's refresh interval");
        reader["hostnames"] = json!([hostname()]);
        reader["services"] = json!([]);
        if let Declarer::Authority = declarer {
            reader
                .as_object_mut()
                .expect("the reader is an object")
                .remove("service_resolver");
        }
        let authority = targets
            .last_mut()
            .expect("the template declares the authority last");
        authority["name"] = json!(AUTHORITY);
        authority["ssh"] = json!(authority_ssh);
        authority["services"] = json!([]);
        if let Declarer::Authority = declarer {
            authority["service_resolver"] = declaration;
        }
        document["service_directory"]["authority"] =
            json!({"target": AUTHORITY, "command": env!("CARGO_BIN_EXE_stado")});
        document["service_directory"]["services"] = json!({});
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        self.date_copy(&chrono::Utc::now().to_rfc3339());
        refresh_seconds
    }

    /// Date the last-known-good copy as read at `read_at`.
    pub(crate) fn date_copy(&self, read_at: &str) {
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({
                "read_at": read_at,
                "generation": "example",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// `stado registry self --name-only`, its stderr kept beside the run as
    /// `<name>.stderr`.
    pub(crate) fn registry_self(&self, name: &str) -> Output {
        let output = self
            .stado()
            .args(["registry", "self", "--name-only"])
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }

    /// The served authority's own log.
    pub(crate) fn authority_log(&self) -> Option<&Path> {
        self.authority.as_ref().map(HungSsh::log)
    }
}

/// Which target of the registry copy declares the resolver whose refresh
/// interval measures the read.
pub(crate) enum Declarer {
    /// This machine, the reader.
    Reader,
    /// The registry authority alone; this machine declares none.
    Authority,
}

/// What a stado command's stderr says when it opened an SSH connection to
/// `destination`: the wait it announces before the connection answers.
pub(crate) fn connecting(destination: &str) -> String {
    format!("czekam: SSH connection; gdzie: {destination};")
}

pub(crate) fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

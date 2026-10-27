//! The commands the deployment runs and how each is kept: the real `stado`
//! as the object API, as a client of it and as a queue on its S3 endpoint,
//! Skarbiec, the AWS CLI, and every run's label, exit status and stderr in
//! report.json.
use serde_json::json;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;

use super::deployment::{Deployment, CONSUMER, KEYS_ROLE};

impl Deployment {
    /// The real `stado` reading the vault as Stado's consumer.
    pub(crate) fn vault_client(&self) -> Command {
        let mut command = self.local();
        command
            .env("WC_SKARBIEC_URL", &self.vault)
            .env("WC_SKARBIEC_CONSUMER", CONSUMER)
            .env("WC_SKARBIEC_TOKEN_FILE", &self.token_file);
        command
    }

    /// The real `stado` keeping its queue on this deployment's S3 endpoint
    /// as the storage backend `backend` (a named provider or s3-compatible),
    /// its key pair read from the vault by [`KEYS_ROLE`].
    pub fn queue_client(&self, backend: &str) -> Command {
        let mut command = self.vault_client();
        command
            .env("WC_STORAGE_BACKEND", backend)
            .env("WC_S3_COMPATIBLE_ENDPOINT", format!("{}/s3", self.origin))
            .env("WC_S3_COMPATIBLE_BUCKET", &self.namespace)
            .env("WC_S3_COMPATIBLE_REGION", "stado")
            .env("WC_S3_COMPATIBLE_ADDRESSING", "path")
            .env("WC_S3_COMPATIBLE_CREDENTIAL_ROLE", KEYS_ROLE);
        command
    }

    pub fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// Where the served store keeps `key` of the namespace.
    pub fn stored(&self, key: &str) -> PathBuf {
        self.store().join("ecosystem").join(&self.namespace).join(key)
    }

    pub(crate) fn skarbiec(&self) -> Command {
        let binary = std::env::var("SKARBIEC")
            .expect("SKARBIEC must name the skarbiec binary under test");
        let mut command = Command::new(binary);
        command
            .env("GNUPGHOME", &self.keyring)
            .env("SKARBIEC_VAULT_FILE", self.root.join("vault.json"))
            .env("SKARBIEC_AUDIT_FILE", self.root.join("audit.jsonl"))
            .stdin(Stdio::null());
        command
    }

    /// The real `stado` in this deployment's home, naming no object API.
    pub(crate) fn local(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    /// The real `stado` as a client of this deployment's object API, with
    /// the namespace's bearer as `STADO_API_TOKEN`.
    pub fn stado(&self) -> Command {
        let mut command = self.local();
        command
            .env("STADO_API_URL", &self.origin)
            .env("STADO_API_TOKEN", &self.bearer);
        command
    }

    /// The AWS CLI signing as `access_key` with `secret`, path-style, at this
    /// deployment's `/s3`, with a configuration of its own: `part` is the
    /// size at and above which, and in which, it uploads in parts.
    pub fn aws(&self, access_key: &str, secret: &str, part: usize) -> Command {
        let config = self.root.join("aws/config");
        let lines = [
            "[default]".to_string(),
            "region = stado".to_string(),
            "s3 =".to_string(),
            "  addressing_style = path".to_string(),
            format!("  multipart_threshold = {part}"),
            format!("  multipart_chunksize = {part}"),
        ];
        fs::write(&config, format!("{}\n", lines.join("\n"))).unwrap();
        let endpoint = format!("{}/s3", self.origin);
        let mut command = Command::new("aws");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("AWS_CONFIG_FILE", &config)
            .env("AWS_SHARED_CREDENTIALS_FILE", self.root.join("aws/credentials"))
            .env("AWS_ACCESS_KEY_ID", access_key)
            .env("AWS_SECRET_ACCESS_KEY", secret)
            .args(["--endpoint-url", &endpoint, "--no-cli-pager"])
            .stdin(Stdio::null());
        command
    }

    /// Run one command and keep its label, exit status and stderr.
    pub fn run(&mut self, label: &str, mut command: Command) -> Output {
        let output = command.output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "command": label,
            "exit_status": output.status.code(),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    /// Run one command that must succeed and answer its stdout.
    pub fn must(&mut self, label: &str, command: Command) -> String {
        let output = self.run(label, command);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{label}: {stderr}");
        String::from_utf8(output.stdout).unwrap()
    }

    /// Run one command that must fail, and answer its stderr.
    pub fn refused(&mut self, label: &str, command: Command) -> String {
        let output = self.run(label, command);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{label} was accepted: {stdout}");
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Start a service and answer the address it printed after `marker`.
    pub(crate) fn spawn(
        &mut self,
        label: &str,
        mut command: Command,
        marker: &'static str,
    ) -> String {
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let log = self.root.join(format!("{label}.stderr"));
        let (sender, announced) = mpsc::channel();
        std::thread::spawn(move || {
            let mut file = File::create(log).unwrap();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                if let Some((_, address)) = line.split_once(marker) {
                    let first = address.split_whitespace().next().unwrap();
                    sender.send(first.to_string()).unwrap();
                }
            }
        });
        self.children.push(child);
        match announced.recv() {
            Ok(address) => address,
            Err(error) => {
                panic!("{label} ended before announcing its address ({error}); see {label}.stderr")
            }
        }
    }

    pub fn save(&self) {
        let report = serde_json::to_vec_pretty(&self.report).unwrap();
        fs::write(self.root.join("report.json"), report).unwrap();
    }
}

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub home: PathBuf,
    pub config: PathBuf,
    pub source: PathBuf,
    pub commit: String,
    pub version: String,
    pub product: String,
    pub consumer_instances: std::collections::BTreeMap<String, String>,
    pub checks: Vec<ConsumerCheck>,
    pub targets: Vec<String>,
    pub api_origin: String,
    pub api_token_file: PathBuf,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerCheck {
    pub binary: String,
    pub arguments: Vec<String>,
    pub expect_version: bool,
}

pub struct Journey {
    pub configuration: Configuration,
    pub output: PathBuf,
    pub report: Value,
    pub manifest: Value,
    pub registry: Value,
    binary: PathBuf,
}

pub fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

impl Journey {
    pub fn new(configuration: Configuration, output: PathBuf) -> Self {
        Self {
            configuration,
            output,
            report: json!({"commands": []}),
            manifest: Value::Null,
            registry: Value::Null,
            binary: PathBuf::from(env!("CARGO_BIN_EXE_stado")),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        fs::write(
            self.output.join("report.json"),
            serde_json::to_vec_pretty(&self.report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn execute(
        &mut self,
        program: &Path,
        arguments: &[&str],
        isolated: bool,
        success: bool,
    ) -> Result<String, String> {
        self.process(Command::new(program).args(arguments), isolated, success)
    }

    fn process(
        &mut self,
        command: &mut Command,
        isolated: bool,
        success: bool,
    ) -> Result<String, String> {
        if isolated {
            command
                .env_clear()
                .env("PATH", std::env::var_os("PATH").ok_or("PATH is missing")?)
                .env("HOME", &self.configuration.home)
                .env("STADO_CONFIG", &self.configuration.config)
                .env("TMPDIR", self.output.join("tmp"));
        }
        let result = command.output();
        let program = command.get_program().to_string_lossy();
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        if let Err(error) = &result {
            self.report["commands"].as_array_mut().unwrap().push(json!({
                "program": program, "arguments": arguments, "isolated": isolated, "spawn_error": error.to_string(),
            }));
            self.save()?;
        }
        let output = result.map_err(|error| format!("{program}: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "program": program, "arguments": arguments, "isolated": isolated,
            "exit_status": output.status.code(), "stdout": stdout, "stderr": stderr,
        }));
        self.save()?;
        require(
            output.status.success() == success,
            &format!(
                "unexpected exit {} for {arguments:?}: {stderr}",
                output.status
            ),
        )?;
        Ok(if success { stdout } else { stderr })
    }

    pub fn cli(&mut self, arguments: &[&str], success: bool) -> Result<Value, String> {
        self.cli_process(self.command().args(arguments), success)
    }

    pub fn command(&self) -> Command {
        Command::new(&self.binary)
    }

    pub fn cli_process(&mut self, command: &mut Command, success: bool) -> Result<Value, String> {
        let output = self.process(command, true, success)?;
        if success {
            serde_json::from_str(&output)
                .map_err(|error| format!("CLI response is not JSON: {error}"))
        } else {
            Ok(json!({"error": output}))
        }
    }

    pub fn objects(
        &mut self,
        namespace: &str,
        prefix: &str,
        isolated: bool,
    ) -> Result<Value, String> {
        let binary = self.binary.clone();
        let output = self.execute(
            &binary,
            &["storage", "objects", namespace, prefix, "--json"],
            isolated,
            true,
        )?;
        serde_json::from_str(&output).map_err(|error| error.to_string())
    }

    pub fn prepare(&mut self) -> Result<(), String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("repository root is missing")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let revision = self.execute(
            Path::new("git"),
            &[
                "-C",
                root.to_str().ok_or("non-UTF8 repository")?,
                "rev-parse",
                "HEAD",
            ],
            false,
            true,
        )?;
        let revision = revision.trim();
        self.execute(
            Path::new("git"),
            &["-C", root.to_str().unwrap(), "diff", "--quiet", "HEAD"],
            false,
            true,
        )?;
        let binary = self.binary.clone();
        let version = self.execute(&binary, &["--version"], false, true)?;
        let reported_revision = version
            .split_whitespace()
            .map(|part| part.trim_matches(['(', ')']))
            .find(|part| part.len() == 40 && part.bytes().all(|byte| byte.is_ascii_hexdigit()));
        require(
            reported_revision == Some(revision),
            "the candidate must identify the exact clean source revision",
        )?;
        let mut file = fs::File::open(&binary).map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        self.report["source_revision"] = json!(revision);
        self.report["binary_sha256"] = json!(format!("{:x}", digest.finalize()));
        self.configuration.home = self
            .configuration
            .home
            .canonicalize()
            .map_err(|error| error.to_string())?;
        self.configuration.config = self
            .configuration
            .config
            .canonicalize()
            .map_err(|error| error.to_string())?;
        self.configuration.source = self
            .configuration
            .source
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let operator_home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        require(
            self.configuration.home != operator_home,
            "the operator home cannot be a test home",
        )?;
        require(
            self.configuration.source.parent() == root.parent(),
            "use the existing canonical product checkout, not a nested checkout",
        )?;
        let source = self
            .configuration
            .source
            .to_str()
            .ok_or("non-UTF8 source")?
            .to_owned();
        let coordinate = format!("{}:.wisent-release.json", self.configuration.commit);
        let manifest = self.execute(
            Path::new("git"),
            &["-C", &source, "show", &coordinate],
            false,
            true,
        )?;
        self.manifest = serde_json::from_str(&manifest).map_err(|error| error.to_string())?;
        require(
            self.manifest["product"] == self.configuration.product,
            "source and configured products differ",
        )?;
        let operator = self.execute(
            &binary,
            &["registry", "pull", "--with-generation"],
            false,
            true,
        )?;
        let operator: Value = serde_json::from_str(&operator).map_err(|error| error.to_string())?;
        let isolated = self.cli(&["registry", "pull", "--with-generation"], true)?;
        let repeated = self.execute(
            &binary,
            &["registry", "pull", "--with-generation"],
            false,
            true,
        )?;
        let repeated: Value = serde_json::from_str(&repeated).map_err(|error| error.to_string())?;
        require(
            operator == repeated
                && isolated["document"] != operator["document"]
                && isolated["generation"] != operator["generation"],
            "test registry must differ from a stable observed operator authority",
        )?;
        let production = operator["document"]["targets"]
            .as_array()
            .ok_or("operator registry targets are unavailable")?;
        let targets = isolated["document"]["targets"]
            .as_array()
            .ok_or("test registry targets are unavailable")?;
        require(
            self.configuration.targets.len() > 1,
            "use multiple real dedicated workers",
        )?;
        for name in &self.configuration.targets {
            let target = targets
                .iter()
                .find(|target| target["name"] == *name)
                .ok_or("a test worker is not registered")?;
            let ssh = target["ssh"]
                .as_str()
                .ok_or("test workers need explicit isolated account identities")?;
            let (user, host) = ssh
                .rsplit_once('@')
                .ok_or("test workers need explicit SSH account names")?;
            require(
                !user.is_empty()
                    && !host.is_empty()
                    && !production.iter().any(|production| {
                        production["ssh"]
                            .as_str()
                            .and_then(|ssh| ssh.rsplit_once('@'))
                            .is_some_and(|(production_user, _)| {
                                user.eq_ignore_ascii_case(production_user)
                            })
                            || target["account_ref"]
                                .as_str()
                                .is_some_and(|account| production["account_ref"] == account)
                    }),
                "use dedicated test account identities, not operator accounts or their aliases",
            )?;
        }
        require(
            isolated["document"]["release_delivery_targets"]
                .get(&self.configuration.product)
                .is_none(),
            "use an isolated product with no preexisting destination declaration",
        )?;
        self.registry = isolated["document"].clone();
        self.report["registry_location"] = isolated["location"].clone();
        self.report["product_commit"] = json!(self.configuration.commit);
        for (namespace, prefix) in [
            (
                "system",
                format!("release-catalog/{}.json", self.configuration.product),
            ),
            ("releases", format!("{}/", self.configuration.product)),
        ] {
            let before = self.objects(namespace, &prefix, false)?;
            let isolated = self.objects(namespace, &prefix, true)?;
            let after = self.objects(namespace, &prefix, false)?;
            require(before == after && before["objects"].as_array().is_some_and(|objects| !objects.is_empty())
                    && isolated["objects"].as_array().is_some_and(Vec::is_empty),
                    "the real catalog and release stores must be empty and separate from stable operator stores")?;
        }
        self.save()
    }
}

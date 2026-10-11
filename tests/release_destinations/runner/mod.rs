use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod prepare;

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
}

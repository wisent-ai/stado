mod instances;
mod parent;
mod settings;
mod wire;
use super::{index::commands::uri, protocol::*, source};
use crate::common::Runtime;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fmt, io, path::PathBuf};

/// Serve the protocol selected by the canonical package source. The
/// protocol owns stdio, so no JSON wrapper shares the connection.
pub fn serve_editor(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let operation = arguments
        .get_one::<String>("operation")
        .context("editor operation is missing")?;
    let package = arguments
        .get_one::<String>("package-path")
        .map(PathBuf::from);
    match operation.as_str() {
        "serve" => {
            if arguments.get_flag("json") {
                bail!(
                    "editor serve owns protocol stdio; --json is only supported by editor status"
                );
            }
            serve(runtime, package)
        }
        "status" => {
            let workspace = package
                .map(|path| {
                    path.canonicalize()
                        .with_context(|| format!("resolve editor workspace {}", path.display()))
                })
                .transpose()?;
            let report = instances::status(runtime, workspace.as_deref())?;
            crate::common::emit(&report)?;
            Ok(libc::EXIT_SUCCESS)
        }
        _ => bail!("unsupported editor operation {operation}"),
    }
}

#[derive(Debug)]
struct ProtocolError {
    code: i32,
    message: String,
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}
impl std::error::Error for ProtocolError {}

struct Server<'a> {
    runtime: &'a Runtime,
    requested: Option<PathBuf>,
    package: Option<PathBuf>,
    settings: settings::Cache,
    instance: Option<instances::Instance>,
    parent_pid: libc::pid_t,
}
impl Server<'_> {
    fn request(&mut self, method: &str, parameters: &Value) -> Result<Value> {
        if method == "build/initialize" {
            let root = parameters["rootUri"]
                .as_str()
                .context("build/initialize requires rootUri")?;
            let url = url::Url::parse(root)?;
            if url.scheme() != "file" || url.host_str().is_some_and(|host| host != "localhost") {
                bail!("native editor workspace must be a local file URI: {root}");
            }
            let requested = self.requested.clone().map(Ok).unwrap_or_else(|| {
                url.to_file_path()
                    .map_err(|_| anyhow::anyhow!("invalid local native workspace URI: {root}"))
            })?;
            let package = source::package(self.runtime, &requested)?.path;
            if let Some(instance) = &self.instance {
                if instance.workspace != package {
                    bail!(
                        "editor connection already owns {}; cannot initialize another workspace {}",
                        instance.workspace.display(),
                        package.display()
                    );
                }
            } else {
                self.instance = Some(instances::Instance::acquire(
                    self.runtime,
                    &package,
                    self.parent_pid,
                )?);
            }
            self.package = Some(package.clone());
            self.settings.invalidate();
            return Ok(
                json!({"displayName": "Wisent canonical native sources", "version": PROVIDER_VERSION,
                "bspVersion": BSP_VERSION, "capabilities": {"buildTargetChangedProvider": true},
                "dataKind": "sourceKit", "data": {"sourceKitOptionsProvider": true, "prepareProvider": false,
                    "indexStorePath": package.join(STATE).join("store"), "indexDatabasePath": package.join(STATE).join("database"),
                    "watchers": [{"globPattern": format!("**/{SETTINGS}")}]}}),
            );
        }
        let package = self.package.as_ref().ok_or_else(|| ProtocolError {
            code: NOT_INITIALIZED,
            message: "build/initialize must select the canonical package first".to_owned(),
        })?;
        match method {
            "build/shutdown" | "workspace/waitForBuildSystemUpdates" => Ok(Value::Null),
            "workspace/buildTargets" => {
                let mut targets = Vec::new();
                for target in self.settings.load(package)?.values() {
                    let toolchain = target.compiler.ancestors().nth(3).with_context(|| {
                        format!(
                            "compiler path has no toolchain directory: {}",
                            target.compiler.display()
                        )
                    })?;
                    targets.push(json!({"id": target.id, "displayName": target.display_name, "baseDirectory": uri(package)?,
                        "tags": [], "languageIds": target.language_ids, "dependencies": target.dependencies,
                        "capabilities": {"canCompile": false, "canTest": false, "canRun": false},
                        "dataKind": "sourceKit", "data": {"toolchain": uri(toolchain)?}}));
                }
                Ok(json!({"targets": targets}))
            }
            "buildTarget/sources" => {
                let targets = self.settings.load(package)?;
                let mut items = Vec::new();
                for identifier in parameters["targets"]
                    .as_array()
                    .context("buildTarget/sources requires targets")?
                {
                    let uri = identifier["uri"].as_str().context("target requires uri")?;
                    let target = targets
                        .get(uri)
                        .with_context(|| format!("unknown native build target: {uri}"))?;
                    items.push(json!({"target": identifier, "sources": target.sources}));
                }
                Ok(json!({"items": items}))
            }
            "textDocument/sourceKitOptions" => {
                let identifier = parameters["target"]["uri"]
                    .as_str()
                    .context("sourceKitOptions requires target.uri")?;
                let document = parameters["textDocument"]["uri"]
                    .as_str()
                    .context("sourceKitOptions requires textDocument.uri")?;
                let Some(target) = self.settings.load(package)?.get(identifier) else {
                    return Ok(Value::Null);
                };
                if !target.sources.iter().any(|source| source.uri == document) {
                    return Ok(Value::Null);
                }
                Ok(
                    json!({"compilerArguments": target.arguments, "workingDirectory": target.working_directory}),
                )
            }
            _ => Err(ProtocolError {
                code: METHOD_NOT_FOUND,
                message: format!("unsupported native build server method: {method}"),
            }
            .into()),
        }
    }
}

pub fn serve(runtime: &Runtime, requested: Option<PathBuf>) -> Result<i32> {
    let parent = parent::Parent::observe()?;
    let instance = match requested.as_ref() {
        Some(requested) => {
            let package = source::package(runtime, requested)?.path;
            Some(instances::Instance::acquire(runtime, &package, parent.pid)?)
        }
        None => None,
    };
    let mut server = Server {
        runtime,
        requested,
        package: None,
        settings: settings::Cache::default(),
        instance,
        parent_pid: parent.pid,
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    while let Some(message) = wire::read(&mut input)? {
        let method = message["method"].as_str().unwrap_or_default();
        if method == "build/exit" {
            return Ok(0);
        }
        let Some(identifier) = message.get("id") else {
            if method == "workspace/didChangeWatchedFiles" && server.package.is_some() {
                server.settings.invalidate();
                wire::write(
                    &mut output,
                    json!({"method": "buildTarget/didChange", "params": {"changes": null}}),
                )?;
            }
            continue;
        };
        let response = match server.request(method, &message["params"]) {
            Ok(result) => json!({"id": identifier, "result": result}),
            Err(error) => {
                let text = format!("{method}: {error:#}");
                eprintln!("{text}");
                wire::write(
                    &mut output,
                    json!({"method": "build/logMessage", "params": {"type": LOG_ERROR, "message": text}}),
                )?;
                let code = error
                    .downcast_ref::<ProtocolError>()
                    .map_or(REQUEST_FAILED, |error| error.code);
                json!({"id": identifier, "error": {"code": code, "message": text}})
            }
        };
        wire::write(&mut output, response)?;
    }
    Ok(0)
}

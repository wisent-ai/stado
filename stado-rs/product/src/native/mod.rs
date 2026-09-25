mod index;
mod protocol;
mod server;
mod source;
mod workspace;
use crate::common::{atomic_json, capture, checked, emit, lock, now, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

struct Execution {
    report: Value,
    stdout: Vec<u8>,
    code: i32,
}

fn execute(
    runtime: &Runtime,
    requested: &Path,
    editor: Option<&Path>,
    operation: &str,
    arguments: &[String],
) -> Result<Execution> {
    for argument in arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--")
    {
        let option = argument.split('=').next().unwrap();
        if matches!(
            option,
            "--package-path"
                | "--scratch-path"
                | "--build-path"
                | "--multiroot-data-file"
                | "--config-path"
                | "--security-path"
                | "--enable-keychain"
        ) {
            bail!("stado product swift owns source resolution; conflicting argument: {argument}");
        }
        if operation == "index"
            && matches!(
                option,
                "--product" | "--target" | "--show-bin-path" | "--print-manifest-job-graph"
            )
        {
            bail!("native index requires the complete package and test graph; conflicting argument: {argument}");
        }
    }
    if operation == "test"
        && !arguments
            .iter()
            .take_while(|argument| argument.as_str() != "--")
            .any(|argument| argument == "--test-product" || argument.starts_with("--test-product="))
    {
        bail!("native test requires --test-product NAME; dependency test suites are not implicitly selected");
    }
    let root = source::package(runtime, requested)?;
    let editor = editor
        .map(crate::common::absolute)
        .transpose()?
        .unwrap_or_else(|| root.path.clone());
    if operation == "index" {
        index::configuration(&editor)?;
    }
    let invocation = uuid::Uuid::new_v4().to_string();
    let output = env::var_os("WISENT_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.root.join(".wisent-output"));
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    let evidence = output.join("native").join(&invocation);
    let parent = root.path.join(".build/wisent-native");
    let _writer = lock(&parent.join("writer.lock"))?;
    let workspace = parent.join(format!("{invocation}.xcworkspace"));
    let scratch = parent.join("build");
    let temporary = parent.join(format!("temporary-{invocation}"));
    fs::create_dir_all(&evidence)?;
    fs::create_dir_all(&temporary)?;
    let mut report = json!({"operation": operation, "package_path": root.path, "scratch_path": scratch,
        "workspace_path": workspace, "editor_workspace": editor, "evidence": evidence, "started_at": now(),
        "stado_version": crate::build().version, "stado_source_revision": crate::build().source_revision, "state": "preparing_sources"});
    atomic_json(&evidence.join("result.json"), &report)?;
    let mut stdout = Vec::new();
    let mut code = 1;
    let result = (|| -> Result<()> {
        report["swift_version"] = json!(String::from_utf8(
            checked(Command::new("swift").arg("--version"))?.stdout
        )?
        .trim());
        report["sources"] = json!(workspace::prepare(runtime, &root, &workspace, &evidence)?);
        let mut argv = vec![
            if operation == "index" {
                "build".to_owned()
            } else {
                operation.to_owned()
            },
            "--package-path".to_owned(),
            root.path.to_string_lossy().into_owned(),
            "--scratch-path".to_owned(),
            scratch.to_string_lossy().into_owned(),
            "--multiroot-data-file".to_owned(),
            workspace.to_string_lossy().into_owned(),
            "--disable-keychain".to_owned(),
        ];
        if operation == "index" {
            argv.extend(index::compiler_arguments(&root.path));
        }
        argv.extend_from_slice(arguments);
        let command = || {
            let mut command = Command::new("swift");
            command
                .args(&argv)
                .current_dir(&root.path)
                .env(workspace::WORKSPACE_ENV, &workspace)
                .env(workspace::SCRATCH_ENV, &scratch)
                .env("TMPDIR", &temporary)
                .env("GIT_ALLOW_PROTOCOL", "")
                .env(
                    "WISENT_SOURCE_COMMIT",
                    root.revision.trim_end_matches("-dirty"),
                );
            command
        };
        report["argv"] = json!(std::iter::once("swift")
            .chain(argv.iter().map(String::as_str))
            .collect::<Vec<_>>());
        report["state"] = json!("building");
        atomic_json(&evidence.join("result.json"), &report)?;
        let output = capture(&mut command())?;
        io::stderr().write_all(&output.stderr)?;
        stdout = output.stdout;
        report["exit_status"] = json!(output.status.code());
        if !output.status.success() {
            code = output.status.code().unwrap_or(1);
            bail!("Swift {operation} failed ({})", output.status);
        }
        report["state"] = json!("verifying_source_identity");
        atomic_json(&evidence.join("result.json"), &report)?;
        for (index, record) in report["sources"]
            .as_array()
            .context("native sources are missing")?
            .iter()
            .enumerate()
        {
            if let Some(source) = record.get("source_snapshot") {
                let root = Path::new(
                    source["repository_path"]
                        .as_str()
                        .context("source record has no repository path")?,
                );
                crate::source::verify_unchanged(
                    root,
                    source,
                    &evidence.join("sources").join(index.to_string()),
                    &temporary,
                )?;
            }
        }
        if operation == "index" {
            report["state"] = json!("publishing_editor_settings");
            atomic_json(&evidence.join("result.json"), &report)?;
            let binary = checked(command().arg("--show-bin-path"))?;
            let binary = PathBuf::from(String::from_utf8(binary.stdout)?.trim());
            report["index"] = index::publish(&root.path, &binary, &report, &editor)?;
        }
        code = 0;
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(&temporary)
        .with_context(|| format!("removing native compiler scratch {}", temporary.display()));
    let result = match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("native scratch cleanup also failed: {cleanup:#}")))
        }
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    };
    report["completed_at"] = json!(now());
    if let Err(error) = result {
        if code == 0 {
            code = 1;
        }
        report["failed_operation"] = report["state"].clone();
        report["state"] = json!("failed");
        report["error"] = json!(format!("{error:#}"));
    } else {
        report["state"] = json!("succeeded");
    }
    report["command_exit_status"] = json!(code);
    atomic_json(&evidence.join("result.json"), &report)?;
    Ok(Execution {
        report,
        stdout,
        code,
    })
}

pub fn run(mut arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let package = arguments
        .remove_one::<String>("package-path")
        .map(PathBuf::from);
    let editor = arguments
        .remove_one::<String>("editor-workspace")
        .map(PathBuf::from);
    let json_output = arguments.get_flag("json");
    let operation = arguments
        .remove_one::<String>("operation")
        .context("Swift operation is missing")?;
    let arguments: Vec<_> = arguments
        .remove_many::<String>("forward")
        .into_iter()
        .flatten()
        .collect();
    if operation == "sourcekit" {
        if json_output || editor.is_some() || !arguments.is_empty() {
            bail!("swift sourcekit is a stdio protocol server; --json, --editor-workspace and extra arguments are not supported");
        }
        return server::serve(runtime, package);
    }
    if editor.is_some() && operation != "index" {
        bail!("--editor-workspace is only supported by swift index");
    }
    let package = package.unwrap_or(env::current_dir()?);
    let execution = execute(runtime, &package, editor.as_deref(), &operation, &arguments)?;
    if json_output {
        emit(&execution.report)?;
    } else {
        io::stdout().write_all(&execution.stdout)?;
        if let Some(error) = execution.report["error"].as_str() {
            eprintln!("{error}");
        }
        eprintln!(
            "Native source and command records: {}",
            execution.report["evidence"].as_str().unwrap()
        );
    }
    Ok(execution.code)
}

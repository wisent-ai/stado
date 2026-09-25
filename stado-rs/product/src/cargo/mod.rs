mod sources;
use crate::common::{atomic_json, capture, checked, emit, now, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
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
    operation: &str,
    arguments: &[String],
) -> Result<Execution> {
    if !matches!(operation, "build" | "check" | "test" | "run" | "metadata") {
        bail!("unknown Cargo operation {operation}");
    }
    for argument in arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--")
    {
        let option = argument.split('=').next().unwrap();
        if matches!(
            option,
            "--manifest-path" | "-m" | "--config" | "--lockfile-path"
        ) {
            bail!("stado product cargo owns source resolution; conflicting argument: {argument}");
        }
    }
    let version = checked(Command::new("cargo").arg("--version"))?;
    let version = String::from_utf8(version.stdout)?.trim().to_owned();
    let mut numbers = version
        .split_whitespace()
        .nth(1)
        .context("Cargo did not report its version")?
        .split('.');
    let major: u32 = numbers
        .next()
        .context("Cargo version has no major")?
        .parse()?;
    let minor: u32 = numbers
        .next()
        .context("Cargo version has no minor")?
        .parse()?;
    if (major, minor) < (1, 97) {
        bail!("canonical Cargo requires 1.97 or later for isolated lockfiles; observed {version}");
    }
    let (manifest, root, identity) = sources::manifest(runtime, requested)?;
    let invocation = uuid::Uuid::new_v4().to_string();
    let output = std::env::var_os("WISENT_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".wisent-output"));
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    let evidence = output.join("cargo").join(&invocation);
    let scratch = root.join(".build/wisent-cargo").join(&invocation);
    fs::create_dir_all(&evidence)?;
    fs::create_dir_all(&scratch)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&evidence, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))?;
    }
    let lockfile = scratch.join("Cargo.lock");
    let mut report = json!({"repository": identity, "manifest_path": manifest, "cargo_version": version,
        "operation": operation, "evidence": evidence, "lockfile_path": lockfile, "started_at": now(),
        "stado_version": crate::build().version, "stado_source_revision": crate::build().source_revision, "state": "preparing_sources"});
    atomic_json(&evidence.join("result.json"), &report)?;
    let mut stdout = Vec::new();
    let mut code = 1;
    let result = (|| -> Result<()> {
        let sources = sources::prepare(runtime, &manifest, &evidence, &scratch)?;
        report["sources"] = json!(sources.records);
        let original = Path::new(
            sources.metadata["workspace_root"]
                .as_str()
                .context("Cargo metadata has no workspace root")?,
        )
        .join("Cargo.lock");
        fs::copy(&original, &lockfile).with_context(|| {
            format!(
                "canonical Cargo requires the source lockfile {}",
                original.display()
            )
        })?;
        let mut options = vec![
            "--manifest-path".to_owned(),
            manifest.to_string_lossy().into_owned(),
            "--offline".to_owned(),
        ];
        options.extend(sources.overrides);
        options.extend([
            "--config".to_owned(),
            format!(
                "resolver.lockfile-path={}",
                serde_json::to_string(&lockfile)?
            ),
        ]);
        let command = |operation: &str| {
            let mut command = Command::new("cargo");
            command
                .arg(operation)
                .args(&options)
                .current_dir(manifest.parent().unwrap())
                .env("GIT_ALLOW_PROTOCOL", "")
                .env("CARGO_NET_OFFLINE", "true")
                .env("CARGO_RESOLVER_LOCKFILE_PATH", &lockfile)
                .env("TMPDIR", &scratch);
            command
        };
        report["state"] = json!("resolving_canonical_sources");
        atomic_json(&evidence.join("result.json"), &report)?;
        let graph = checked(command("metadata").args(["--format-version", "1"]))?;
        let graph: Value = serde_json::from_slice(&graph.stdout)?;
        for package in graph["packages"]
            .as_array()
            .context("resolved Cargo graph has no packages")?
        {
            if package["source"]
                .as_str()
                .is_some_and(|s| s.starts_with("git+"))
            {
                bail!(
                    "Cargo retained a Git dependency instead of canonical source: {}",
                    package["id"]
                );
            }
            if package["source"].is_null() {
                sources::manifest(
                    runtime,
                    Path::new(
                        package["manifest_path"]
                            .as_str()
                            .context("resolved Cargo package has no manifest")?,
                    ),
                )?;
            }
        }
        atomic_json(&evidence.join("resolved-graph.json"), &graph)?;
        fs::copy(&lockfile, evidence.join("Cargo.lock"))?;
        let mut process = command(operation);
        process.arg("--locked");
        if operation == "metadata" {
            process.args(["--format-version", "1"]);
        }
        process.args(arguments);
        report["argv"] = json!(std::iter::once(process.get_program())
            .chain(process.get_args())
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>());
        report["state"] = json!("executing");
        atomic_json(&evidence.join("result.json"), &report)?;
        let output = capture(&mut process)?;
        io::stderr().write_all(&output.stderr)?;
        stdout = output.stdout;
        report["exit_status"] = json!(output.status.code());
        report["process_status"] = json!(output.status.to_string());
        if !output.status.success() {
            code = output.status.code().unwrap_or(1);
            bail!("Cargo {operation} failed ({})", output.status);
        }
        report["state"] = json!("verifying_source_identity");
        atomic_json(&evidence.join("result.json"), &report)?;
        for (index, source) in sources.records.iter().enumerate() {
            let root = Path::new(
                source["repository_path"]
                    .as_str()
                    .context("source record has no repository path")?,
            );
            crate::source::verify_unchanged(
                root,
                source,
                &evidence.join("sources").join(index.to_string()),
                &scratch,
            )?;
        }
        code = 0;
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(&scratch)
        .with_context(|| format!("removing private Cargo scratch {}", scratch.display()));
    let result = match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Cargo scratch cleanup also failed: {cleanup:#}")))
        }
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    };
    report["completed_at"] = json!(now());
    if let Err(error) = &result {
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
    let manifest = arguments
        .remove_one::<String>("manifest-path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    let json_output = arguments.get_flag("json");
    let operation = arguments
        .remove_one::<String>("operation")
        .context("Cargo operation is missing")?;
    let arguments: Vec<_> = arguments
        .remove_many::<String>("forward")
        .into_iter()
        .flatten()
        .collect();
    let execution = execute(runtime, &manifest, &operation, &arguments)?;
    if json_output {
        emit(&execution.report)?;
    } else {
        io::stdout().write_all(&execution.stdout)?;
        if let Some(error) = execution.report["error"].as_str() {
            eprintln!("{error}");
        }
        eprintln!(
            "Cargo source and command records: {}",
            execution.report["evidence"].as_str().unwrap()
        );
    }
    Ok(execution.code)
}

pub(crate) mod private_sources;
mod sources;
mod stage;
use crate::common::{atomic_json, capture, checked, emit, now, toolchain_command, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) struct Execution {
    pub(crate) report: Value,
    stdout: Vec<u8>,
    pub(crate) code: i32,
}

/// Run `operation` against the manifest's checkout as it stands.
pub(crate) fn execute(
    runtime: &Runtime,
    requested: &Path,
    operation: &str,
    arguments: &[String],
) -> Result<Execution> {
    execute_revision(runtime, requested, None, operation, arguments)
}

/// Run `operation` against the manifest's checkout, or, with `revision`,
/// against that committed revision of it exported into the checkout's build
/// area. `operation` is one the CLI's own value set admitted
/// (`cli::native::cargo`); nothing here decides it again.
pub(crate) fn execute_revision(
    runtime: &Runtime,
    requested: &Path,
    revision: Option<&str>,
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
            "--manifest-path" | "-m" | "--config" | "--lockfile-path"
        ) {
            bail!("stado product cargo owns source resolution; conflicting argument: {argument}");
        }
    }
    if std::env::var_os("WISENT_SOURCE_DIR").is_some() {
        return private_sources::execute(runtime, requested, operation, arguments);
    }
    let version = checked(toolchain_command("cargo").arg("--version"))?;
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
    // A pinned revision: the product's committed tree, exported into its
    // checkout's build area, is what the manifest is read from and built;
    // the checkout's working tree - every session's half-written edit in it
    // - is not.
    let pinned = match revision {
        Some(commit) => Some(sources::pin(runtime, requested, commit)?),
        None => None,
    };
    let (manifest, root, identity) = match &pinned {
        Some(pinned) => {
            let (checkout_manifest, _, _) = sources::manifest(runtime, requested)?;
            let relative = checkout_manifest
                .strip_prefix(&pinned.checkout)
                .context("the manifest is not under its checkout")?;
            (
                pinned.exported.join(relative),
                pinned.checkout.clone(),
                pinned.identity.clone(),
            )
        }
        None => sources::manifest(runtime, requested)?,
    };
    let invocation = uuid::Uuid::new_v4().to_string();
    // The release worker declares its output inside the job tree it sweeps
    // with the job, so its scratch stays in that tree too. Run by hand,
    // evidence and scratch go to the checkout's build area under Stado's
    // home, never into the checkout, which sits in `~/Documents` where no
    // janitor pass reaches.
    let (output, scratch_parent) = match std::env::var_os("WISENT_OUTPUT_DIR") {
        Some(declared) => (PathBuf::from(declared), root.join(".build/wisent-cargo")),
        None => {
            let area = crate::common::runs::checkout_area(&runtime.home, &root)?;
            (area.clone(), area.join("cargo-scratch"))
        }
    };
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    let evidence = output.join("cargo").join(&invocation);
    let scratch = scratch_parent.join(&invocation);
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
        let sources = sources::prepare(runtime, pinned.as_ref(), &manifest, &evidence, &scratch)?;
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
            let mut command = toolchain_command("cargo");
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
        // Registry crates this host never downloaded would stop the offline
        // resolution below. They are fetched first into Cargo's own cache,
        // against the same private lockfile, with Git transport still off, so
        // only registry sources whose checksums the lockfile pins can arrive.
        report["state"] = json!("fetching_registry_crates");
        atomic_json(&evidence.join("result.json"), &report)?;
        checked(
            toolchain_command("cargo")
                .arg("fetch")
                .args(
                    options
                        .iter()
                        .filter(|option| option.as_str() != "--offline"),
                )
                .current_dir(manifest.parent().unwrap())
                .env("GIT_ALLOW_PROTOCOL", "")
                .env("CARGO_RESOLVER_LOCKFILE_PATH", &lockfile)
                .env("TMPDIR", &scratch),
        )?;
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
                sources::manifest_in(
                    runtime,
                    pinned.as_ref(),
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
        // Every compiling operation runs through the declared compiler cache,
        // so a crate this host compiled for any earlier build — another run
        // directory, another product — is restored instead of compiled again.
        if operation != "metadata" {
            report["state"] = json!("ensuring_compiler_cache");
            atomic_json(&evidence.join("result.json"), &report)?;
            let declaration = crate::compiler_cache::declaration()?;
            let wrapper = crate::compiler_cache::ensure(&runtime.home)?;
            report["compiler_cache"] = wrapper.report(&declaration);
            process.env("RUSTC_WRAPPER", &wrapper.path);
        }
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
            // A pinned export is a committed tree: its identity is the
            // commit and archive digest its marker records, read again.
            if source["pinned"].as_bool() == Some(true) {
                let exported = Path::new(
                    source["exported_path"]
                        .as_str()
                        .context("pinned source record has no exported path")?,
                );
                let marker: Value =
                    serde_json::from_slice(&fs::read(exported.join(".stado-exported.json"))?)?;
                if marker["archive_sha256"] != source["archive_sha256"]
                    || marker["commit"] != source["commit"]
                {
                    bail!(
                        "the pinned export {} changed under the build: its marker no longer \
                         names commit {} at archive {}",
                        exported.display(),
                        source["commit"],
                        source["archive_sha256"]
                    );
                }
                continue;
            }
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
    let revision = arguments.remove_one::<String>("revision");
    let operation = arguments
        .remove_one::<String>("operation")
        .context("Cargo operation is missing")?;
    let arguments: Vec<_> = arguments
        .remove_many::<String>("forward")
        .into_iter()
        .flatten()
        .collect();
    if operation == "stage" {
        return stage::stage(
            runtime,
            &manifest,
            revision.as_deref(),
            &arguments,
            json_output,
        );
    }
    let execution = execute_revision(
        runtime,
        &manifest,
        revision.as_deref(),
        &operation,
        &arguments,
    )?;
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

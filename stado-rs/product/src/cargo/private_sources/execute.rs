use super::configure;
use crate::cargo::Execution;
use crate::common::{atomic_json, capture, checked, now, sha256, toolchain_command, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(in crate::cargo) fn execute(
    runtime: &Runtime,
    requested: &Path,
    operation: &str,
    arguments: &[String],
) -> Result<Execution> {
    let source = std::env::var_os("WISENT_SOURCE_DIR").context("WISENT_SOURCE_DIR is required")?;
    let root = fs::canonicalize(&source)
        .with_context(|| format!("cannot read WISENT_SOURCE_DIR={source:?}"))?;
    let manifest = fs::canonicalize(requested)
        .with_context(|| format!("cannot read Cargo manifest {}", requested.display()))?;
    if !manifest.starts_with(&root) {
        bail!(
            "Cargo manifest {} is outside WISENT_SOURCE_DIR {}",
            manifest.display(),
            root.display()
        );
    }
    let located = checked(
        toolchain_command("cargo")
            .args([
                "locate-project",
                "--workspace",
                "--message-format",
                "plain",
                "--manifest-path",
            ])
            .arg(&manifest)
            .current_dir(&root),
    )?;
    let workspace_manifest = fs::canonicalize(String::from_utf8(located.stdout)?.trim())
        .context("cannot resolve the Cargo workspace manifest")?;
    let workspace = workspace_manifest
        .parent()
        .context("Cargo workspace manifest has no parent")?;
    if !workspace.starts_with(&root) {
        bail!(
            "Cargo workspace {} is outside WISENT_SOURCE_DIR {}",
            workspace.display(),
            root.display()
        );
    }
    let evidence = runtime
        .output
        .join("cargo")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&evidence)?;
    let mut command = toolchain_command("cargo");
    command
        .arg(operation)
        .arg("--manifest-path")
        .arg(&manifest)
        .current_dir(workspace)
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true");
    configure(&mut command, workspace)?;
    if !arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| matches!(argument.as_str(), "--locked" | "--frozen"))
    {
        command.arg("--locked");
    }
    command.args(arguments);
    let mut report = json!({
        "operation": operation, "manifest_path": manifest, "evidence": evidence,
        "workspace_root": workspace,
        "cargo_lock_sha256": sha256(&workspace.join("Cargo.lock"))?,
        "source_revision": crate::build().source_revision,
        "started_at": now(), "state": "running",
    });
    if matches!(operation, "build" | "check" | "test" | "run") {
        let declaration = crate::compiler_cache::declaration()?;
        let wrapper = crate::compiler_cache::ensure(&runtime.home)?;
        command.env("RUSTC_WRAPPER", &wrapper.path);
        report["compiler_cache"] = wrapper.report(&declaration);
    }
    report["argv"] = json!(format!("{command:?}"));
    atomic_json(&evidence.join("result.json"), &report)?;
    let output = capture(&mut command);
    report["finished_at"] = json!(now());
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            report["state"] = json!("failed");
            report["error"] = json!(format!("{error:#}"));
            atomic_json(&evidence.join("result.json"), &report)?;
            return Err(error);
        }
    };
    report["exit_status"] = json!(output.status.code());
    report["state"] = json!(if output.status.success() {
        "succeeded"
    } else {
        "failed"
    });
    if !output.status.success() {
        report["error"] = json!(format!(
            "Cargo {operation} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    atomic_json(&evidence.join("result.json"), &report)?;
    io::stderr().write_all(&output.stderr)?;
    let code = output
        .status
        .code()
        .with_context(|| format!("Cargo ended without an exit code: {}", output.status))?;
    Ok(Execution {
        report,
        stdout: output.stdout,
        code,
    })
}

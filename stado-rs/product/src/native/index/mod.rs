pub mod commands;
use super::protocol::{
    BSP_VERSION, CONNECTION, PROVIDER_VERSION, SETTINGS, SETTINGS_SCHEMA, STATE,
};
use crate::common::atomic_json;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub fn compiler_arguments(package: &Path) -> Vec<String> {
    let store = package
        .join(STATE)
        .join("store")
        .to_string_lossy()
        .into_owned();
    vec![
        "--build-tests".into(),
        "-Xswiftc".into(),
        "-index-store-path".into(),
        "-Xswiftc".into(),
        store.clone(),
        "-Xcc".into(),
        "-index-store-path".into(),
        "-Xcc".into(),
        store,
    ]
}

pub fn configuration(workspace: &Path) -> Result<Value> {
    for directory in [workspace.join(".bsp"), workspace.join(".sourcekit-lsp")] {
        if directory
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            bail!(
                "editor configuration directory is a symlink: {}",
                directory.display()
            );
        }
    }
    let bsp = workspace.join(".bsp");
    if bsp.is_dir() {
        for entry in fs::read_dir(bsp)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && path != workspace.join(CONNECTION)
            {
                bail!(
                    "another editor build server is already declared in {}; not replacing it",
                    workspace.display()
                );
            }
        }
    }
    if workspace.join("buildServer.json").exists() {
        bail!(
            "another editor build server is already declared in {}; not replacing it",
            workspace.display()
        );
    }
    let path = workspace.join(".sourcekit-lsp/config.json");
    for path in [&path, &workspace.join(CONNECTION)] {
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            bail!("editor configuration is a symlink: {}", path.display());
        }
    }
    let value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };
    if !value.is_object() {
        bail!(
            "editor configuration must be a JSON object: {}",
            path.display()
        );
    }
    Ok(value)
}

pub fn publish(package: &Path, binary: &Path, report: &Value, editor: &Path) -> Result<Value> {
    let mut configuration = configuration(editor)?;
    let scratch = Path::new(
        report["scratch_path"]
            .as_str()
            .context("native report has no scratch path")?,
    );
    let manifest = scratch.join(format!(
        "{}.yaml",
        binary
            .file_name()
            .context("Swift binary path has no configuration name")?
            .to_string_lossy()
    ));
    let targets = commands::targets(package, &manifest, &report["sources"])?;
    let settings = json!({"schema_version": SETTINGS_SCHEMA, "package_path": package,
        "sources": report["sources"], "native_evidence": report["evidence"], "targets": targets});
    atomic_json(&package.join(SETTINGS), &settings)?;
    let mut argv = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "product".to_owned(),
        "swift".to_owned(),
    ];
    if editor != package {
        argv.extend([
            "--package-path".to_owned(),
            package.to_string_lossy().into_owned(),
        ]);
    }
    argv.push("sourcekit".to_owned());
    atomic_json(
        &editor.join(CONNECTION),
        &json!({"name": "wisent-native", "version": PROVIDER_VERSION,
        "bspVersion": BSP_VERSION, "argv": argv, "languages": ["c", "cpp", "objective-c", "objective-cpp", "swift"]}),
    )?;
    configuration["defaultWorkspaceType"] = json!("buildServer");
    atomic_json(&editor.join(".sourcekit-lsp/config.json"), &configuration)?;
    Ok(
        json!({"settings": package.join(SETTINGS), "manifest": manifest, "connection": editor.join(CONNECTION),
        "targets": targets.len(), "sources": targets.iter().map(|target| target["sources"].as_array().map_or(0, Vec::len)).sum::<usize>(),
        "index_store": package.join(STATE).join("store")}),
    )
}

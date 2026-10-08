//! Where Cargo builds on a host whose janitor may not read a folder that holds
//! checkouts.
//!
//! A checkout's `target/` is a tagged cache this cleaner reclaims, but only
//! where the janitor may read. macOS keeps `~/Documents`, where the fleet's
//! checkouts live, from a program the operator did not grant it, so every
//! build there grew a tree no pass could reach while the disk filled. When a
//! consent-gated folder refuses this process, Cargo's build directory — the
//! intermediate artifacts, the bulk of every build — is declared in Cargo's
//! own configuration under its home, which Cargo tags with `CACHEDIR.TAG` and
//! this cleaner reclaims. A configuration that already declares a build
//! directory is the operator's choice and is left as it is; one that exists
//! without it is not rewritten, and the pass says so.

use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::{consent, CleanupReport, JanitorError};

/// Cargo's own template for a per-workspace build directory under its home:
/// https://doc.rust-lang.org/cargo/reference/config.html#buildbuild-dir
const BUILD_DIR: &str = "{cargo-cache-home}/build/{workspace-path-hash}";
/// The key Cargo reads the build directory from.
const BUILD_DIR_KEY: &str = "build-dir";
/// One pass's verdict on the placement is one skip of its reason.
const ONE_VERDICT: i64 = 1; // https://doc.rust-lang.org/std/primitive.i64.html

/// Cargo's configuration file in the home Cargo itself reads.
fn cargo_config(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".cargo"),
    }
    .join("config.toml")
}

/// A consent-gated folder this process may not read, if there is one.
fn refused_folder(home: &Path) -> Option<PathBuf> {
    consent::gated_folders(home).into_iter().find(|folder| {
        std::fs::read_dir(folder).is_err_and(|error| {
            cfg!(target_os = "macos") && error.raw_os_error() == Some(nix::libc::EPERM)
        })
    })
}

fn write_declaration(config: &Path, folder: &Path) -> std::io::Result<()> {
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        config,
        format!(
            "# Written by the Stado janitor: {} refuses it, so checkouts there build\n\
             # into Cargo's home, where a pass can reclaim the tagged tree.\n\
             [build]\n{BUILD_DIR_KEY} = \"{BUILD_DIR}\"\n",
            folder.display()
        ),
    )
}

pub(super) fn place_cargo_builds(home: &Path, report: &mut CleanupReport) {
    let Some(folder) = refused_folder(home) else {
        return;
    };
    let config = cargo_config(home);
    let outcome = match std::fs::read_to_string(&config) {
        Ok(text)
            if text
                .lines()
                .any(|line| line.trim_start().starts_with(BUILD_DIR_KEY)) =>
        {
            Ok("cargo_build_dir_declared")
        }
        Ok(_) => Ok("cargo_config_without_build_dir"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_declaration(&config, &folder)
                .map(|()| "cargo_build_dir_placed")
                .map_err(|error| format!("cannot write {}: {error}", config.display()))
        }
        Err(error) => Err(format!("cannot read {}: {error}", config.display())),
    };
    match outcome {
        Ok(reason) => report.skip_builds(reason, ONE_VERDICT),
        Err(detail) => report.add_error("build_caches", &JanitorError::os(&detail)),
    }
}

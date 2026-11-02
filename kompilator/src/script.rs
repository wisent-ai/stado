//! A package's build script, run the way the packages that ship one expect:
//! in the package's directory, with the environment Cargo documents for
//! build scripts, and its standard output read for the instructions it gives
//! the compilation of its package (`cargo:` and `cargo::` lines).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a build script told the build.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub cfgs: Vec<String>,
    pub envs: Vec<(String, String)>,
    /// `-l` values for this package's compilation.
    pub libs: Vec<String>,
    /// `-L` values for this package and every package above it.
    pub searches: Vec<String>,
    /// `-C link-arg` values for this package's binaries.
    pub link_args: Vec<String>,
    /// `cargo:KEY=VALUE` of a package with `links`, given to the build
    /// scripts of the packages that depend on it as `DEP_<LINKS>_<KEY>`.
    pub metadata: Vec<(String, String)>,
    /// Files whose change runs the script again (`rerun-if-changed`).
    pub rerun_files: Vec<String>,
    /// Variables whose change runs the script again (`rerun-if-env-changed`).
    pub rerun_env: Vec<String>,
    /// What the script asked to show (`warning`).
    pub warnings: Vec<String>,
}

/// Read the instructions in a build script's standard output.
pub fn read(stdout: &str) -> Output {
    let mut output = Output::default();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:")) else { continue };
        let Some((key, value)) = rest.split_once('=') else { continue };
        let value = value.trim().to_string();
        match key {
            "rustc-cfg" => output.cfgs.push(value),
            "rustc-env" => {
                if let Some((name, value)) = value.split_once('=') {
                    output.envs.push((name.to_string(), value.to_string()));
                }
            }
            "rustc-link-lib" => output.libs.push(value),
            "rustc-link-search" => output.searches.push(value),
            "rustc-link-arg" | "rustc-link-arg-bins" => output.link_args.push(value),
            "rustc-flags" => {
                let mut words = value.split_whitespace().peekable();
                while let Some(word) = words.next() {
                    let (flag, inline) = word.split_at(word.len().min(2));
                    let argument = if inline.is_empty() { words.next().map(String::from) } else { Some(inline.to_string()) };
                    match (flag, argument) {
                        ("-l", Some(argument)) => output.libs.push(argument),
                        ("-L", Some(argument)) => output.searches.push(argument),
                        _ => {}
                    }
                }
            }
            "metadata" => {
                if let Some((name, value)) = value.split_once('=') {
                    output.metadata.push((name.to_string(), value.to_string()));
                }
            }
            "rerun-if-changed" => output.rerun_files.push(value),
            "rerun-if-env-changed" => output.rerun_env.push(value),
            "warning" => output.warnings.push(value),
            "rustc-check-cfg" | "error" | "rustc-cdylib-link-arg" | "rustc-link-arg-tests" | "rustc-link-arg-examples"
            | "rustc-link-arg-benches" | "rustc-link-arg-cdylib" | "rustc-link-arg-bin" => {}
            other => output.metadata.push((other.to_string(), value)),
        }
    }
    output
}

/// The environment one build script runs with.
pub struct Environment<'a> {
    pub manifest: &'a crate::manifest::Manifest,
    pub out_dir: &'a Path,
    pub features: &'a std::collections::BTreeSet<String>,
    pub target: &'a crate::cfg::Target,
    pub rustc: &'a str,
    pub profile_release: bool,
    pub jobs: usize,
    /// `DEP_<LINKS>_<KEY>` values from the packages it depends on.
    pub dep_metadata: &'a BTreeMap<String, String>,
}

/// Run the compiled build script `binary`; its instructions, or why it failed
/// with what it printed.
pub fn run(binary: &Path, environment: &Environment) -> Result<Output, String> {
    let manifest = environment.manifest;
    std::fs::create_dir_all(environment.out_dir)
        .map_err(|error| format!("{} cannot be made: {error}", environment.out_dir.display()))?;
    let mut command = Command::new(binary);
    command.current_dir(&manifest.dir);
    command.env("OUT_DIR", environment.out_dir);
    command.env("CARGO_MANIFEST_DIR", &manifest.dir);
    command.env("CARGO_MANIFEST_PATH", manifest.dir.join("Cargo.toml"));
    if let Some(links) = &manifest.links {
        command.env("CARGO_MANIFEST_LINKS", links);
    }
    package_env(&mut command, manifest);
    command.env("TARGET", &environment.target.triple);
    command.env("HOST", &environment.target.triple);
    command.env("NUM_JOBS", environment.jobs.to_string());
    command.env("OPT_LEVEL", if environment.profile_release { "3" } else { "0" });
    command.env("DEBUG", if environment.profile_release { "false" } else { "true" });
    command.env("PROFILE", if environment.profile_release { "release" } else { "debug" });
    command.env("RUSTC", environment.rustc);
    command.env("RUSTDOC", "rustdoc");
    command.env("CARGO_ENCODED_RUSTFLAGS", "");
    let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in &environment.target.cfg {
        let entry = keys.entry(key.clone()).or_default();
        if let Some(value) = value {
            entry.push(value.clone());
        }
    }
    for (key, values) in keys {
        command.env(format!("CARGO_CFG_{}", key.to_uppercase()), values.join(","));
    }
    for feature in environment.features {
        command.env(format!("CARGO_FEATURE_{}", feature.to_uppercase().replace('-', "_")), "1");
    }
    for (key, value) in environment.dep_metadata {
        command.env(key, value);
    }
    let output = command.output().map_err(|error| format!("the build script of {} could not run: {error}", manifest.name))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    std::fs::write(environment.out_dir.join("../script-output.txt"), &stdout).ok();
    if !output.status.success() {
        return Err(format!(
            "the build script of {} {} exited {}:\n{}\n{}",
            manifest.name,
            manifest.version,
            output.status,
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(read(&stdout))
}

/// The `CARGO_PKG_*` variables of `manifest`, which crates read with `env!`.
pub fn package_env(command: &mut Command, manifest: &crate::manifest::Manifest) {
    command.env("CARGO_PKG_NAME", &manifest.name);
    command.env("CARGO_PKG_VERSION", &manifest.version);
    let core = manifest.version.split(['-', '+']).next().unwrap_or(&manifest.version);
    let mut parts = core.split('.');
    command.env("CARGO_PKG_VERSION_MAJOR", parts.next().unwrap_or("0"));
    command.env("CARGO_PKG_VERSION_MINOR", parts.next().unwrap_or("0"));
    command.env("CARGO_PKG_VERSION_PATCH", parts.next().unwrap_or("0"));
    let pre = manifest.version.split_once('-').map(|(_, pre)| pre.split('+').next().unwrap_or(pre)).unwrap_or("");
    command.env("CARGO_PKG_VERSION_PRE", pre);
    for (key, variable) in [
        ("description", "CARGO_PKG_DESCRIPTION"),
        ("homepage", "CARGO_PKG_HOMEPAGE"),
        ("repository", "CARGO_PKG_REPOSITORY"),
        ("license", "CARGO_PKG_LICENSE"),
        ("rust-version", "CARGO_PKG_RUST_VERSION"),
        ("readme", "CARGO_PKG_README"),
        ("authors", "CARGO_PKG_AUTHORS"),
    ] {
        command.env(variable, manifest.metadata.get(key).map(String::as_str).unwrap_or(""));
    }
    command.env("CARGO_PKG_LICENSE_FILE", "");
    command.env("CARGO", std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kompilator")));
}

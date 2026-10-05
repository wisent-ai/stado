use super::{atomic_json, now, sha256, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{
    fs::{self, File},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::LazyLock,
};

static EXECUTABLE_HASH: LazyLock<std::result::Result<String, String>> = LazyLock::new(|| {
    std::env::current_exe()
        .context("locating the running Stado executable")
        .and_then(|path| sha256(&path))
        .map_err(|error| format!("{error:#}"))
});

fn recorded(command: &mut Command) -> Result<(Output, PathBuf)> {
    let runtime = Runtime::new(None)?;
    let run = super::runs::fresh_record(
        &runtime.output.join("commands"),
        &uuid::Uuid::new_v4().to_string(),
    )?;
    let folder = run.path.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
    }
    let stdout = folder.join("stdout.log");
    let stderr = folder.join("stderr.log");
    command
        .stdout(Stdio::from(File::create(&stdout)?))
        .stderr(Stdio::from(File::create(&stderr)?));
    let mut report = json!({
        "started_at": now(), "program": command.get_program().to_string_lossy(),
        "args": command.get_args().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "cwd": command.get_current_dir().map(|p| p.to_string_lossy()),
        "source_revision": crate::build().source_revision,
        "executable_sha256": EXECUTABLE_HASH.as_ref().ok(), "state": "starting"
    });
    let record = folder.join(super::runs::COMMAND_RECORD);
    atomic_json(&record, &report)?;
    if let Err(error) = &*EXECUTABLE_HASH {
        report["state"] = json!("failed");
        report["error"] = json!(error);
        report["finished_at"] = json!(now());
        atomic_json(&record, &report)?;
        bail!(
            "cannot attest the running Stado executable: {error}; evidence {}",
            folder.display()
        );
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report["state"] = json!("failed");
            report["error"] = json!(error.to_string());
            report["finished_at"] = json!(now());
            atomic_json(&record, &report)?;
            return Err(error).with_context(|| {
                format!(
                    "starting {:?}; evidence {}",
                    command.get_program(),
                    folder.display()
                )
            });
        }
    };
    report["pid"] = json!(child.id());
    report["state"] = json!("running");
    let running_record = atomic_json(&record, &report);
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            report["state"] = json!("failed");
            report["error"] = json!(error.to_string());
            report["finished_at"] = json!(now());
            atomic_json(&record, &report)?;
            return Err(error).with_context(|| {
                format!(
                    "waiting for {:?}; evidence {}",
                    command.get_program(),
                    folder.display()
                )
            });
        }
    };
    report["state"] = json!(if status.success() {
        "succeeded"
    } else {
        "failed"
    });
    report["exit_status"] = json!(status.code());
    report["process_status"] = json!(status.to_string());
    report["finished_at"] = json!(now());
    if let Err(error) = &running_record {
        report["recording_error"] = json!(format!("{error:#}"));
    }
    atomic_json(&record, &report)?;
    running_record.with_context(|| {
        format!(
            "recording running command {:?}; observed {status}; evidence {}",
            command.get_program(),
            folder.display()
        )
    })?;
    let output = Output {
        status,
        stdout: fs::read(&stdout).with_context(|| format!("reading {}", stdout.display()))?,
        stderr: fs::read(&stderr).with_context(|| format!("reading {}", stderr.display()))?,
    };
    Ok((output, folder))
}

pub fn capture(command: &mut Command) -> Result<Output> {
    recorded(command).map(|(output, _)| output)
}

/// A command that ran and exited unsuccessfully, with its exit code kept so a
/// caller can tell what kind of failure it was; its text is what `checked`
/// always reported.
#[derive(Debug)]
pub struct CommandFailed {
    pub code: Option<i32>,
    message: String,
}

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandFailed {}

pub fn checked(command: &mut Command) -> Result<Output> {
    let (output, evidence) = recorded(command)?;
    if !output.status.success() {
        return Err(CommandFailed {
            code: output.status.code(),
            message: format!(
                "{:?} failed ({}); evidence {}: {}{}",
                command.get_program(),
                output.status,
                evidence.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        }
        .into());
    }
    Ok(output)
}

/// Resolve one quality or build program the way the host actually carries it.
///
/// A LaunchAgent's PATH is minimal by design (`/opt/homebrew/bin:...:/bin`),
/// and the Rust toolchain installs itself into `~/.cargo/bin`: a step that
/// names a bare `cargo` dies `No such file or directory (os error 2)` inside
/// the host's own agent while it runs over an SSH channel, whose login shell
/// carries a fuller PATH. A relative program is looked up in the homes Stado
/// and the toolchains install into first; a name none of them carries falls
/// through to the spawn's own PATH lookup, so a correctly provisioned PATH
/// keeps working unchanged. The release worker and `stado product install`
/// resolve their steps here, so a recipe runs the same program under both.
pub fn step_program(program: &str) -> PathBuf {
    let path = std::path::Path::new(program);
    if path.is_absolute() || program.contains('/') {
        return path.to_path_buf();
    }
    // A candidate must be an executable file, as `execvp` requires. uv's
    // installer writes `~/.local/bin/env`, a shell snippet meant to be
    // sourced; where that file comes first it shadows `/usr/bin/env`.
    step_search_directories()
        .into_iter()
        .map(|directory| directory.join(program))
        .find(|candidate| executable_file(candidate))
        .unwrap_or_else(|| path.to_path_buf())
}

/// A command for a toolchain program Stado starts itself, `cargo` above all:
/// the program resolved by [`step_program`] and a PATH that puts the same
/// directories ahead of the inherited one, so the program's own children
/// (`rustc`, a `RUSTC_WRAPPER`, a build script's tools) resolve there too.
/// A bare `Command::new("cargo")` finds nothing on the host agent's minimal
/// PATH, which is how the compiler-cache install failed every darwin build.
pub fn toolchain_command(program: &str) -> Command {
    let mut command = Command::new(step_program(program));
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    if let Some(path) = step_search_path(None, &inherited) {
        command.env("PATH", path);
    }
    command
}

/// The directories [`step_program`] looks a bare program up in, in order:
/// the owner-only installs (`stado` itself and the fleet's delivered
/// binaries) first, then the toolchains' homes.
///
/// A step's own children need them too: a step that is a script calling
/// `cargo` found nothing on the LaunchAgent's minimal PATH and exited 127,
/// while the same `cargo` named as the step's program was found here. A
/// caller that builds a step's environment puts these ahead of the
/// inherited PATH ([`step_search_path`]).
pub fn step_search_directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = std::path::Path::new(&home);
        directories.push(home.join(".stado").join("bin"));
        directories.push(home.join(".local").join("bin"));
        directories.push(home.join(".cargo").join("bin"));
    }
    directories.push(PathBuf::from("/opt/homebrew/bin"));
    directories.push(PathBuf::from("/usr/local/bin"));
    directories
}

/// `first`, then [`step_search_directories`], then `inherited`, each
/// directory once: the PATH a step and every program it starts resolve
/// against.
pub fn step_search_path(
    first: Option<PathBuf>,
    inherited: &std::ffi::OsStr,
) -> Option<std::ffi::OsString> {
    let mut directories: Vec<PathBuf> = Vec::new();
    for directory in first
        .into_iter()
        .chain(step_search_directories())
        .chain(std::env::split_paths(inherited))
    {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    std::env::join_paths(directories).ok()
}

fn executable_file(candidate: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    candidate
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

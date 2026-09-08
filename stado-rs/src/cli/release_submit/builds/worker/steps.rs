//! Running one declared step on the builder: where its program lives, what
//! its receipt says, and the toolchain components its gates demand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;
use crate::release_pipeline::{StepReceipt, StepStatus};

/// Resolve one quality/build program the way the agent host actually carries
/// it.
///
/// A LaunchAgent's PATH is minimal by design (`/opt/homebrew/bin:...:/bin`),
/// and the Rust toolchain installs itself into `~/.cargo/bin` — which is how
/// the first stado release job in this fleet's history died: `cargo` existed
/// on the host, the agent's PATH could not see it, and the bare `?` reported
/// `No such file or directory (os error 2)` without naming what it was trying
/// to run. A relative program is looked up here first; a name none of the
/// known homes carries falls through to the spawn's own PATH lookup, so a
/// correctly provisioned PATH keeps working unchanged.
fn resolve_step_program(program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.is_absolute() || program.contains('/') {
        return path.to_path_buf();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        // The owner-only installs first: `stado` itself and the fleet's
        // delivered binaries live here, and the first delivery that ran
        // `stado release install-local` died unable to find the very
        // program that had been delivered to this directory.
        candidates.push(Path::new(&home).join(".stado").join("bin").join(program));
        candidates.push(Path::new(&home).join(".local").join("bin").join(program));
        candidates.push(Path::new(&home).join(".cargo").join("bin").join(program));
    }
    candidates.push(Path::new("/opt/homebrew/bin").join(program));
    candidates.push(Path::new("/usr/local/bin").join(program));
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| path.to_path_buf())
}

pub(crate) fn execute(
    name: &str,
    argv: &[String],
    source: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<StepReceipt, CmdError> {
    let program = resolve_step_program(&argv[0]);
    // The step's start is logged before the spawn, so a step that hangs or
    // dies leaves its name and argv in the job output instead of silence.
    println!(
        "[release-worker] step {name}: {} {}",
        program.display(),
        argv[1..].join(" ")
    );
    let status = Command::new(&program)
        .args(&argv[1..])
        .current_dir(source)
        .envs(environment)
        .status()
        .map_err(|error| {
            CmdError::click(format!(
                "step {name}: cannot run {}: {error}",
                program.display()
            ))
        })?;
    println!("[release-worker] step {name}: exit {:?}", status.code());
    Ok(StepReceipt {
        name: name.into(),
        argv: argv.to_vec(),
        status: if status.success() {
            StepStatus::Passed
        } else {
            StepStatus::Failed
        },
        exit_code: status.code(),
    })
}

/// Install the toolchain components this recipe's gates run, when the recipe
/// is a Rust one and rustup manages the host's toolchain.
///
/// Scoped deliberately: only `cargo fmt` needs `rustfmt` and only
/// `cargo clippy` needs `clippy`, and a recipe that runs neither provisions
/// nothing. rustup reads the toolchain pin from the working directory, so the
/// component lands on exactly the toolchain the gate will use. A host without
/// rustup is left alone — its cargo is not rustup-managed and components are
/// not its concept.
pub(super) fn ensure_rust_components(
    recipe: &crate::release_pipeline::PlatformRecipe,
    source: &Path,
) -> Result<(), CmdError> {
    let mut needed: Vec<&str> = Vec::new();
    for gate in &recipe.quality {
        let program = gate.argv.first().map(String::as_str).unwrap_or("");
        let subcommand = gate.argv.get(1).map(String::as_str).unwrap_or("");
        if program == "cargo" || program.ends_with("/cargo") {
            match subcommand {
                "fmt" if !needed.contains(&"rustfmt") => needed.push("rustfmt"),
                "clippy" if !needed.contains(&"clippy") => needed.push("clippy"),
                _ => {}
            }
        }
    }
    if needed.is_empty() {
        return Ok(());
    }
    let rustup = resolve_step_program("rustup");
    if !rustup.is_absolute() || !rustup.is_file() {
        println!(
            "[release-worker] no rustup on this host; assuming {} are already provided",
            needed.join(", ")
        );
        return Ok(());
    }
    println!(
        "[release-worker] ensuring toolchain components: {}",
        needed.join(", ")
    );
    let output = Command::new(&rustup)
        .arg("component")
        .arg("add")
        .args(&needed)
        .current_dir(source)
        .output()
        .map_err(|error| CmdError::click(format!("cannot run {}: {error}", rustup.display())))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "rustup component add {} failed: {}",
            needed.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

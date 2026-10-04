//! Running one declared step on the builder: where its program lives, what
//! its receipt says, and the toolchain components its gates demand.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use stado_product::common::step_program as resolve_step_program;

use crate::cli::CmdError;
use crate::release_pipeline::{StepReceipt, StepStatus};

/// A step written `env NAME=VALUE… program args…` sets variables for one
/// program. Run through `env` itself, the program is looked up on the
/// LaunchAgent's minimal PATH, which `resolve_step_program` exists to avoid:
/// a step written this way stops with `env: cargo: No such file or directory`
/// while every step naming `cargo` directly finds it. So the assignments
/// become the step's environment and the program after them is resolved like
/// any other.
fn split_env_prefix(argv: &[String]) -> (&[String], BTreeMap<String, String>) {
    let mut assignments = BTreeMap::new();
    if Path::new(&argv[0])
        .file_name()
        .and_then(|name| name.to_str())
        != Some("env")
    {
        return (argv, assignments);
    }
    let mut rest = &argv[1..];
    while let Some((name, value)) = rest.first().and_then(|word| word.split_once('=')) {
        if name.is_empty() || name.starts_with('-') {
            break;
        }
        assignments.insert(name.to_string(), value.to_string());
        rest = &rest[1..];
    }
    if rest.is_empty() || rest[0].starts_with('-') {
        // `env` with no program, or with its own options: run it as written.
        return (argv, BTreeMap::new());
    }
    (rest, assignments)
}

pub(crate) fn execute(
    name: &str,
    argv: &[String],
    source: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<StepReceipt, CmdError> {
    let (command, assignments) = split_env_prefix(argv);
    let program = resolve_step_program(&command[0]);
    // The step's start is logged before the spawn, so a step that hangs or
    // dies leaves its name and argv in the job output instead of silence.
    // The start time and the exit's duration are what `stado build status`
    // reads back while the job runs: which step it is in, since when, and
    // what every finished step cost.
    println!(
        "[release-worker] step {name}: {} {}",
        program.display(),
        command[1..].join(" ")
    );
    println!(
        "[release-worker] step {name}: started at {}",
        chrono::Utc::now().to_rfc3339()
    );
    let started = std::time::Instant::now();
    let status = Command::new(&program)
        .args(&command[1..])
        .current_dir(source)
        .envs(environment)
        .envs(&assignments)
        .status()
        .map_err(|error| {
            CmdError::click(format!(
                "step {name}: cannot run {}: {error}",
                program.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    println!(
        "[release-worker] step {name}: exit {:?} after {}s",
        status.code(),
        started.elapsed().as_secs()
    );
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
/// Refuse a build the host has no room for, before the first crate.
///
/// A release build that runs out of space fails after every minute it was
/// going to spend: a build that compiles hundreds of crates and then dies on
/// `No space left on device (os error 28)` writing rustc metadata leaves the
/// cause as one line inside a long log. The
/// requirement is the recipe's own (`min_free_gb`), the observation is the
/// work volume's, and a recipe that declares nothing is not measured.
pub(super) fn require_free_space(
    recipe: &crate::release_pipeline::PlatformRecipe,
    work: &Path,
) -> Result<(), CmdError> {
    if recipe.min_free_gb == 0 {
        return Ok(());
    }
    let free = free_gibibytes(work).ok_or_else(|| {
        CmdError::click(format!(
            "this build declares {} GiB of free space and the free space of {} could not be read",
            recipe.min_free_gb,
            work.display()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if free < recipe.min_free_gb as f64 {
        return Err(CmdError::refused(format!(
            "this build needs {} GiB free on {} and the volume has {free:.1} GiB; \
             reclaim space on this host (`stado space reclaim <host> --apply --reason …`) \
             or move the work root, then submit again",
            recipe.min_free_gb,
            work.display()
        )));
    }
    Ok(())
}

/// Free space of the volume holding `path`, in GiB, from the file system's
/// own `statvfs` answer: blocks available to an unprivileged writer times the
/// block size. The two figures are widened to `u128` because their width
/// differs between platforms, and the product is read back as `f64`.
fn free_gibibytes(path: &Path) -> Option<f64> {
    let stats = nix::sys::statvfs::statvfs(path).ok()?;
    let bytes = u128::from(stats.blocks_available()) * u128::from(stats.fragment_size());
    Some(bytes as f64 / crate::providers::local::disk_cleanup::GIB as f64)
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
        .map_err(|error| {
            CmdError::click(format!("cannot run {}: {error}", rustup.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "rustup component add {} failed: {}",
            needed.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}

//! Running one declared step on the builder: where its program lives, what
//! its receipt says, and the toolchain components its gates demand.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use stado_product::common::step_program;

use crate::cli::CmdError;
use crate::release_pipeline::{StepReceipt, StepStatus};

/// A step written `env NAME=VALUE… program args…` sets variables for one
/// program. Run through `env` itself, the program is looked up on the
/// LaunchAgent's minimal PATH, which [`step_program`] exists to avoid:
/// a step written this way stops with `env: cargo: No such file or directory`
/// while every step naming `cargo` directly finds it. So the assignments
/// become the step's environment and the program after them is resolved like
/// any other.
fn split_env_prefix(argv: &[String]) -> (&[String], BTreeMap<String, String>) {
    let mut assignments = BTreeMap::new();
    let Some((program, mut rest)) = argv.split_first() else {
        return (argv, assignments);
    };
    if Path::new(program).file_name() != Some(OsStr::new("env")) {
        return (argv, assignments);
    }
    while let Some((word, after)) = rest.split_first() {
        let Some((name, value)) = word.split_once('=') else {
            break;
        };
        if name.is_empty() || name.starts_with('-') {
            break;
        }
        assignments.insert(name.to_string(), value.to_string());
        rest = after;
    }
    match rest.first() {
        Some(program) if !program.starts_with('-') => (rest, assignments),
        // `env` with no program, or with its own options: run it as written.
        _ => (argv, BTreeMap::new()),
    }
}

pub(crate) fn execute(
    name: &str,
    argv: &[String],
    source: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<StepReceipt, CmdError> {
    let (command, assignments) = split_env_prefix(argv);
    let Some((named, arguments)) = command.split_first() else {
        return Err(
            CmdError::click(format!("step {name} declares an empty argv"))
                .stating(crate::primitives::failure::FailureCode::Config),
        );
    };
    // The program is looked up on the PATH the step runs with: its own `env`
    // assignment, else the worker's step environment, else this process's.
    let path = match (assignments.get("PATH"), environment.get("PATH")) {
        (Some(own), _) => Some(OsString::from(own)),
        (None, Some(worker)) => Some(OsString::from(worker)),
        (None, None) => std::env::var_os("PATH"),
    };
    let program = step_program(named, path.as_deref()).map_err(|error| {
        CmdError::click(format!("step {name}: {error:#}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    // The step's start is logged before the spawn, so a step that hangs or
    // dies leaves its name and argv in the job output instead of silence.
    // The start time and the exit's duration are what `stado build status`
    // reads back while the job runs: which step it is in, since when, and
    // what every finished step cost.
    println!(
        "[release-worker] step {name}: {} {}",
        program.display(),
        arguments.join(" ")
    );
    println!(
        "[release-worker] step {name}: started at {}",
        chrono::Utc::now().to_rfc3339()
    );
    let started = std::time::Instant::now();
    let status = crate::wait::status(
        Command::new(&program)
            .args(arguments)
            .current_dir(source)
            .envs(environment)
            .envs(&assignments),
    )
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
    let Some(required) = std::num::NonZeroU64::new(recipe.min_free_gb) else {
        return Ok(());
    };
    let free = free_gibibytes(work).map_err(|error| {
        CmdError::click(format!(
            "this build declares {required} GiB of free space and the free space of {} could not be read: {error}",
            work.display()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if free < required.get() as f64 {
        return Err(CmdError::refused(format!(
            "this build needs {required} GiB free on {} and the volume has {free:.1} GiB; \
             reclaim space on this host (`stado space reclaim <host> --apply --reason …`) \
             or move the work root, then submit again",
            work.display()
        )));
    }
    Ok(())
}

/// The file whose presence at the source's root makes it a Node source.
const NODE_MANIFEST: &str = "package.json";
/// The Node interpreter every step of a Node source ends up running.
const NODE_INTERPRETER: &str = "node";
/// The package runner Node release scripts call (`npx prettier --check`, …).
const NODE_RUNNER: &str = "npx";

/// Refuse a Node source on a builder that carries no Node toolchain, before
/// the first step.
///
/// A builder without Node ran the gates anyway and stopped inside a gate
/// script with `exec: npx: not found` (exit 127), after the other platforms
/// had qualified, so the release held only some of its platforms and the
/// cause was one line of a gate's output. The lookup walks the steps' own
/// PATH, so a program the steps would find is one this finds.
pub(super) fn require_node_toolchain(
    source: &Path,
    environment: &BTreeMap<String, String>,
    platform: &str,
) -> Result<(), CmdError> {
    if !source.join(NODE_MANIFEST).is_file() {
        return Ok(());
    }
    let Some(path) = environment.get("PATH") else {
        return Err(CmdError::refused(format!(
            "this {platform} build is a Node source ({NODE_MANIFEST}) and its step environment \
             carries no PATH to find `{NODE_INTERPRETER}` on"
        )));
    };
    for program in [NODE_INTERPRETER, NODE_RUNNER] {
        if let Err(error) = step_program(program, Some(OsStr::new(path))) {
            return Err(CmdError::refused(format!(
                "this {platform} build is a Node source ({NODE_MANIFEST}) and this builder has \
                 no `{program}` for its steps ({error:#}); give the {platform} platform a \
                 `runner_platform` whose host carries the Node toolchain, or install Node on \
                 this builder, then submit again"
            )));
        }
    }
    Ok(())
}

/// Free space of the volume holding `path`, in GiB, from the file system's
/// own `statvfs` answer: blocks available to an unprivileged writer times the
/// block size. The two figures are widened to `u128` because their width
/// differs between platforms, and the product is read back as `f64`.
fn free_gibibytes(path: &Path) -> Result<f64, nix::Error> {
    let stats = nix::sys::statvfs::statvfs(path)?;
    let bytes = u128::from(stats.blocks_available()) * u128::from(stats.fragment_size());
    Ok(bytes as f64 / crate::providers::local::space_reclaim::GIB as f64)
}

/// Whether the unpacked source is a Cargo one: a `Cargo.toml` at its root or
/// in a directory directly under it (Stado keeps its workspace in
/// `stado-rs/`). Its steps then compile through the compiler cache. A source
/// that cannot be listed is refused: read as not a Cargo source, its build
/// ran without the compiler cache and nothing said why.
pub(super) fn cargo_source(source: &Path) -> Result<bool, CmdError> {
    if source.join("Cargo.toml").is_file() {
        return Ok(true);
    }
    let unlisted = |error: std::io::Error| {
        CmdError::click(format!("cannot list {}: {error}", source.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    };
    for entry in std::fs::read_dir(source).map_err(unlisted)? {
        if entry.map_err(unlisted)?.path().join("Cargo.toml").is_file() {
            return Ok(true);
        }
    }
    Ok(false)
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
        let Some((program, arguments)) = gate.argv.split_first() else {
            continue;
        };
        if Path::new(program).file_name() != Some(OsStr::new("cargo")) {
            continue;
        }
        // The rustup component that carries the Cargo subcommand a gate runs.
        let component = match arguments.first().map(String::as_str) {
            Some("fmt") => "rustfmt",
            Some("clippy") => "clippy",
            _ => continue,
        };
        if !needed.contains(&component) {
            needed.push(component);
        }
    }
    if needed.is_empty() {
        return Ok(());
    }
    let path = std::env::var_os("PATH");
    let rustup = match step_program("rustup", path.as_deref()) {
        Ok(rustup) => rustup,
        Err(error) => {
            println!(
                "[release-worker] {error:#}; this host's cargo is not rustup-managed, so {} are taken as provided",
                needed.join(", ")
            );
            return Ok(());
        }
    };
    println!(
        "[release-worker] ensuring toolchain components: {}",
        needed.join(", ")
    );
    let output = crate::wait::output(
        Command::new(&rustup)
            .arg("component")
            .arg("add")
            .args(&needed)
            .current_dir(source),
    )
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

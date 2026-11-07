//! One declared compiling gate as a fleet host runs it: a Cargo-shaped gate
//! (`cargo build --bin …|check|clippy|test`) as `stado host cargo` does, any
//! other exactly as the release manifest declares its argv, run in the
//! delivered tree's root (`host_run::gate`). A `cargo test` gate that
//! selects no target is cut into one shard per test target by [`targets`].
//!
//! [`targets`]: super::targets

use std::path::Path;

use crate::cli::change::git::git;
use crate::cli::change::record::Shard;
use crate::cli::CmdError;
use crate::deploy::host_run::{validate_cargo, validate_gate, CargoOperation, HostCargoRequest};
use crate::release_pipeline::{GateKind, QualityGate, PRODUCT_MANIFEST};

/// What a host runs for one shard.
#[derive(Clone)]
pub enum Work {
    /// A Cargo operation, run as `stado host cargo` runs it.
    Cargo {
        operation: CargoOperation,
        binaries: Vec<String>,
        arguments: Vec<String>,
    },
    /// A gate that is no Cargo operation: its argv, run as declared.
    Declared(Vec<String>),
}

/// One unit of gate work for one host of one platform.
#[derive(Clone)]
pub struct Planned {
    pub gate: String,
    pub kind: GateKind,
    pub platform: String,
    /// The manifest the gate names, relative to the checkout: its
    /// `Cargo.toml`, or the release manifest for a declared gate.
    pub manifest: String,
    pub work: Work,
}

impl Planned {
    /// The gate as it runs, for records and the action cache's key.
    pub fn argv(&self) -> Vec<String> {
        let (operation, binaries, arguments) = match &self.work {
            Work::Declared(argv) => return argv.clone(),
            Work::Cargo {
                operation,
                binaries,
                arguments,
            } => (operation, binaries, arguments),
        };
        let mut argv = vec![
            "cargo".to_string(),
            operation.word().to_string(),
            "--manifest-path".to_string(),
            self.manifest.clone(),
        ];
        for binary in binaries {
            argv.extend(["--bin".to_string(), binary.clone()]);
        }
        argv.extend(arguments.iter().cloned());
        argv
    }
}

/// `gate` of `platform` as the work a fleet host runs.
pub fn placed(gate: &QualityGate, platform: &str) -> Planned {
    // A declared gate is run beside the release manifest it is read from.
    let (manifest, work) = cargo_shaped(&gate.argv).unwrap_or_else(|| {
        (
            PRODUCT_MANIFEST.to_string(),
            Work::Declared(gate.argv.clone()),
        )
    });
    Planned {
        gate: gate.name.clone(),
        kind: gate.kind,
        platform: platform.to_string(),
        manifest,
        work,
    }
}

/// `argv` as the manifest and the work of a Cargo operation host cargo
/// runs, or none when it is not one.
fn cargo_shaped(argv: &[String]) -> Option<(String, Work)> {
    let mut words = argv.iter();
    if words.next().map(String::as_str) != Some("cargo") {
        return None;
    }
    let operation = match words.next().map(String::as_str) {
        Some("build") => CargoOperation::Build,
        Some("check") => CargoOperation::Check,
        Some("clippy") => CargoOperation::Clippy,
        Some("test") => CargoOperation::Test,
        _ => return None,
    };
    let (mut manifest, mut binaries, mut arguments) =
        ("Cargo.toml".to_string(), Vec::new(), Vec::new());
    let mut passthrough = false;
    while let Some(word) = words.next() {
        if passthrough {
            arguments.push(word.clone());
            continue;
        }
        match word.as_str() {
            "--" => {
                passthrough = true;
                arguments.push(word.clone());
            }
            "--locked" => {}
            "--release" if operation == CargoOperation::Build => {}
            "--manifest-path" => manifest = words.next()?.clone(),
            "--bin" if operation == CargoOperation::Build => binaries.push(words.next()?.clone()),
            other => match other.strip_prefix("--manifest-path=") {
                Some(path) => manifest = path.to_string(),
                None => arguments.push(word.clone()),
            },
        }
    }
    if operation == CargoOperation::Build && binaries.is_empty() {
        return None;
    }
    let work = Work::Cargo {
        operation,
        binaries,
        arguments,
    };
    Some((manifest, work))
}

/// Whether a host can be given `planned` at `commit` at all: a manifest the
/// commit does not hold, an argument Stado owns or an empty argv is refused
/// by every host alike, so it is the candidate's refusal (`Err` of the
/// inner result), never a host that cannot run its shard.
pub async fn runnable(
    checkout: &Path,
    commit: &str,
    planned: &Planned,
) -> Result<Result<(), String>, CmdError> {
    let refused = |why: String| {
        format!(
            "gate {} ({}) for {} cannot run: {why}",
            planned.gate, planned.kind, planned.platform
        )
    };
    let listed = git(
        checkout,
        &["ls-tree", "--name-only", commit, "--", &planned.manifest],
    )
    .await?;
    if listed.is_empty() {
        return Ok(Err(refused(format!(
            "{commit} holds no {}",
            planned.manifest
        ))));
    }
    let manifest = checkout.join(&planned.manifest);
    let checked = match &planned.work {
        Work::Declared(argv) => validate_gate(&manifest, argv),
        Work::Cargo {
            operation,
            binaries,
            arguments,
        } => validate_cargo(&HostCargoRequest {
            manifest_path: manifest,
            revision: commit.to_string(),
            operation: *operation,
            binaries: binaries.clone(),
            arguments: arguments.clone(),
        }),
    };
    Ok(checked.map_err(|error| refused(error.to_string())))
}

impl Planned {
    /// This gate as the pass records it when the operator's rulings left its
    /// action out: never run, on no host, with the ruling Oko answered.
    pub fn left_out(&self, ruling: &str) -> Shard {
        Shard {
            gate: self.gate.clone(),
            kind: self.kind.to_string(),
            platform: self.platform.clone(),
            argv: self.argv(),
            host: "none".to_string(),
            cached: false,
            exit_code: -1,
            seconds: 0,
            report: String::new(),
            left_out: Some(ruling.to_string()),
        }
    }
}

/// The shard of every gate of `compiling` whose action `ruling` names the
/// operator's ruling for: each left out, never run.
pub fn left_out<'a>(
    compiling: &[Planned],
    ruling: impl Fn(&Planned) -> Option<&'a String>,
) -> Vec<Shard> {
    compiling
        .iter()
        .filter_map(|planned| ruling(planned).map(|said| planned.left_out(said)))
        .collect()
}

/// The admission asks a pass makes for a candidate of `product` repairing
/// `defects`, in the words it runs them with (`allowed::words`): for each
/// action its queued gates take, as the fleet's daily build and, when a
/// session submitted it, as that session's work.
pub fn asks(product: &str, defects: &[String], session: Option<&str>) -> String {
    let purposes: Vec<&str> = defects.iter().map(String::as_str).collect();
    let mut asked = Vec::new();
    for action in super::QUEUED.iter().map(|kind| super::action(*kind)) {
        // The fleet's ask, then the submitter's when a session submitted it.
        for whose in std::iter::once(None).chain(session.map(Some)) {
            let words =
                crate::cli::release_loop::allowed::words(action, Some(product), &purposes, whose);
            asked.push(format!("`oko {}`", words.join(" ")));
        }
    }
    asked.join(" and ")
}

/// Why a pass refuses the candidate: every remote shard that refused, with
/// its argv, host, exit and whole report; `None` when none did.
pub fn refusal(remote: &[Shard]) -> Option<String> {
    let refused: Vec<String> = remote
        .iter()
        .filter(|shard| shard.exit_code != 0)
        .map(|shard| {
            format!(
                "gate {} ({}) for {} on {} refused, `{}` exited {}:\n{}",
                shard.gate,
                shard.kind,
                shard.platform,
                shard.host,
                shard.argv.join(" "),
                shard.exit_code,
                shard.report
            )
        })
        .collect();
    (!refused.is_empty()).then(|| refused.join("\n\n"))
}

/// The compiling gates of `shards` that ran or the action cache answered,
/// named as [`super::Plan::names`] names them, when the rulings left any
/// out; `None` when none was, so the risk classified from the plan stands.
pub fn ran(shards: &[Shard]) -> Option<Vec<String>> {
    if shards.iter().all(|shard| shard.left_out.is_none()) {
        return None;
    }
    let mut names: Vec<String> = shards
        .iter()
        .filter(|shard| shard.left_out.is_none() && shard.kind != super::SOURCE_KIND)
        .map(|shard| format!("{} ({}) for {}", shard.gate, shard.kind, shard.platform))
        .collect();
    names.dedup();
    Some(names)
}

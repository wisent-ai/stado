//! `stado product surface`: does the next tag of a Swift package agree with
//! what the change did to its public API?
//!
//! The released surface is the toolchain's dump of the library module built
//! from the tag consumers resolve; the candidate is the same dump of the
//! working tree. `swift api-digester` names what the candidate breaks. The
//! declared version is the version tag pointing at the working revision, and
//! when none does, the released version — still what a consumer resolving
//! `from:` gets — so that claim is only honest while the API is unchanged.
//! The command never bumps and never tags.

mod digest;
mod released;

use crate::common::{absolute, emit, Runtime};
use anyhow::{bail, Context, Result};
use released::{Artifact, Version};
use serde_json::{json, Value};
use std::path::Path;

/// What the candidate did to the released surface, decided from the
/// digester's verdict and the dumps' equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Internal,
    Additive,
    Breaking,
}

impl Change {
    fn name(self) -> &'static str {
        match self {
            Change::Internal => "internal",
            Change::Additive => "additive",
            Change::Breaking => "breaking",
        }
    }

    /// The next version the change requires. Before 1.0 a breaking change
    /// moves the minor number, as SwiftPM's `from:` treats 0.x minors as
    /// incompatible.
    fn next(self, released: Version) -> Version {
        match self {
            Change::Internal => Version {
                patch: released.patch + 1,
                ..released
            },
            Change::Additive => Version {
                minor: released.minor + 1,
                patch: 0,
                ..released
            },
            Change::Breaking if released.major == 0 => Version {
                minor: released.minor + 1,
                patch: 0,
                ..released
            },
            Change::Breaking => Version {
                major: released.major + 1,
                minor: 0,
                patch: 0,
            },
        }
    }
}

fn surfaces_equal(baseline: &Path, candidate: &Path) -> Result<bool> {
    let read = |path: &Path| -> Result<Value> {
        serde_json::from_str(&std::fs::read_to_string(path)?)
            .with_context(|| format!("{} is not JSON", path.display()))
    };
    Ok(read(baseline)? == read(candidate)?)
}

fn measure(baseline: &Path, candidate: &Path, report: &mut Value) -> Result<Change> {
    let (diagnosis, broke) = digest::diagnose(baseline, candidate)?;
    report["diagnosis"] = json!(diagnosis);
    if broke {
        return Ok(Change::Breaking);
    }
    if surfaces_equal(baseline, candidate)? {
        return Ok(Change::Internal);
    }
    Ok(Change::Additive)
}

/// The verdict as the gate consumes it: `Ok` when the declared version agrees
/// with the change, an error naming the required tag otherwise.
fn verdict(change: Change, released: Version, declared: Option<Version>) -> Result<String> {
    let required = change.next(released);
    match declared.filter(|declared| *declared != released) {
        None => {
            if change != Change::Internal {
                bail!(
                    "the public API changed ({}) but no tag above {released} points at this revision, so every package resolving a `from:` requirement keeps getting {released} while its surface has moved. The next tag must be {required}; this check does not create it",
                    change.name()
                );
            }
            Ok(format!("no new tag, and the API is unchanged since {released}"))
        }
        Some(declared) if declared != required => bail!(
            "tag {declared} points at this revision, but a {} change to {released} requires {required}",
            change.name()
        ),
        Some(declared) => Ok(format!("tag {declared} matches the {} change", change.name())),
    }
}

fn check(package: &Path, module: Option<&str>, report: &mut Value) -> Result<String> {
    let module = match module {
        Some(module) => module.to_owned(),
        None => digest::library_module(package)?,
    };
    report["module"] = json!(module);
    let scratch = package.join(".build/wisent-surface");
    std::fs::create_dir_all(&scratch)?;
    let candidate_dump = scratch.join("candidate.json");
    let modules = digest::build(package, &scratch.join("candidate"))?;
    digest::dump(&module, &modules, &candidate_dump)?;
    report["candidate"] = json!(candidate_dump);

    let (artifact, released) = released::artifact(package)?;
    report["released"] = match &artifact {
        Artifact::Release { tag } => json!({"kind": "release", "tag": tag}),
        Artifact::Tag { tag } => json!({"kind": "tag", "tag": tag}),
        Artifact::Nothing => json!({"kind": "nothing"}),
    };
    let Some((tag, released)) = artifact.tag().map(str::to_owned).zip(released) else {
        report["change"] = json!("initial");
        return Ok("neither a tag nor a release exists at the remote; nothing is released and there is no contract to compare against".to_owned());
    };
    let tree = released::checkout_tag(package, &tag, &scratch.join("released-tree"))?;
    let baseline_dump = scratch.join("released.json");
    let modules = digest::build(&tree, &scratch.join("released"))?;
    digest::dump(&module, &modules, &baseline_dump)?;
    report["baseline"] = json!(baseline_dump);
    let change = measure(&baseline_dump, &candidate_dump, report)?;
    let declared = released::declared(package)?;
    report["change"] = json!(change.name());
    report["released_version"] = json!(released.to_string());
    report["declared_version"] = json!(declared.map(|version| version.to_string()));
    report["required_version"] = json!(change.next(released).to_string());
    verdict(change, released, declared)
}

/// Run `stado product surface`, returning the exit status.
pub fn run(arguments: &clap::ArgMatches, _runtime: &Runtime) -> Result<i32> {
    let package = arguments
        .get_one::<String>("package-path")
        .map_or_else(std::env::current_dir, |path| absolute(Path::new(path)))?;
    let module = arguments.get_one::<String>("module").map(String::as_str);
    let json_output = arguments.get_flag("json");
    let mut report = json!({"package_path": package, "stado_version": crate::build().version,
        "stado_source_revision": crate::build().source_revision});
    let outcome = check(&package, module, &mut report);
    let code = match &outcome {
        Ok(message) => {
            report["state"] = json!("agreed");
            report["message"] = json!(message);
            0
        }
        Err(error) => {
            report["state"] = json!("refused");
            report["error"] = json!(format!("{error:#}"));
            1
        }
    };
    if json_output {
        emit(&report)?;
    } else {
        if let Some(diagnosis) = report["diagnosis"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
        {
            eprintln!("{diagnosis}");
        }
        match &outcome {
            Ok(message) => println!("{message}"),
            Err(error) => eprintln!("stado product surface: {error:#}"),
        }
    }
    Ok(code)
}

//! Embed the repository's enrollment bootstrap script and the source revision
//! this binary was built from.
//!
//! `GET /join.sh` on the dashboard must hand the joining machine exactly the
//! script that lives in the repository at `deploy/join.sh` — the script is not
//! written in Rust and is never templated. The served binary has to carry it,
//! because the dashboard runs from an installed binary with no repository
//! checkout beside it. A missing script is not a build failure: the route
//! answers 503 when the copy is empty, which is what happens in build contexts
//! whose source tree does not include `deploy/`.
//!
//! # Why the revision is embedded
//!
//! Nothing in a built Stado used to say which tree produced it, so the only
//! answer to "which build is this" was the semantic version -- and that does
//! not identify content. On 2026-09-03 `0.14.6` named four materially
//! different trees: the binary deployed on the fleet (missing the janitor
//! workload-hold fix and the builder-claimability fix), two separate commits
//! that each declare `version = "0.14.6"` in `Cargo.toml`, and a local build
//! carrying a fourth combination. No release object existed for `0.14.6` to
//! disambiguate them, only a coordinate claim. Establishing what the running
//! control plane actually carried took reading string literals and mangled
//! symbols out of the binary with `strings` and `nm`. This makes it a read.
//!
//! A build that cannot find a revision must still build. A tree with no git
//! metadata, a source tarball, or a CI checkout without history is a normal
//! way to build this crate, and refusing there would be worse than the defect
//! it guards against: it would take away the only route that currently ships
//! fixes. Each context therefore has a defined answer, in this order:
//!
//! 1. `STADO_SOURCE_REVISION` in the environment wins. A caller building a
//!    source tarball can state the revision explicitly with the variable the
//!    binary consumes.
//! 2. Otherwise `WISENT_SOURCE_COMMIT` names the release pipeline's verified
//!    source snapshot. The worker already exports that identity; the source
//!    archive deliberately carries no `.git` directory.
//! 3. Otherwise `git rev-parse` names it, with `-dirty` appended when the
//!    working tree carries uncommitted changes. The dirty marker is the point:
//!    the deployed `0.14.6` above is exactly what an uncommitted local build
//!    installed by hand looks like, and a revision alone would have claimed
//!    more precision than the bytes deserve.
//! 4. Otherwise [`UNKNOWN_REVISION`]. Honest, greppable, and never a panic.
//!
//! One limitation, stated rather than hidden: the rerun triggers below fire on
//! a commit, a checkout and a branch switch, but cargo cannot watch "the whole
//! working tree", so editing a tracked file without committing does not by
//! itself re-stamp an already-built binary. Any edit that recompiles this
//! crate re-runs the script and re-stamps it.

use std::path::Path;
use std::process::Command;

/// What the revision reads as when no context could name one. The consumer
/// treats this as a value, not an error, so `stado --version` still answers.
const UNKNOWN_REVISION: &str = "unknown";

/// The variable consumed by builds that explicitly state Stado's revision.
const REVISION_OVERRIDE: &str = "STADO_SOURCE_REVISION";

/// The verified source identity exported by Stado's release worker.
const PIPELINE_REVISION: &str = "WISENT_SOURCE_COMMIT";

/// Twelve hex digits: short enough to read aloud, long enough that this
/// repository will not collide.
const REVISION_LENGTH: &str = "--short=12";

/// Run `git` in the repository root and return trimmed stdout, or `None` when
/// git is absent, this is not a repository, or the command fails for any other
/// reason. Nothing here is allowed to panic or to fail the build.
fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir("..")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// The revision this build should claim, by the order documented above.
fn source_revision() -> String {
    println!("cargo:rerun-if-env-changed={REVISION_OVERRIDE}");
    println!("cargo:rerun-if-env-changed={PIPELINE_REVISION}");
    for name in [REVISION_OVERRIDE, PIPELINE_REVISION] {
        if let Some(stated) = std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return stated;
        }
    }

    // Re-stamp when HEAD moves. `.git/HEAD` covers a checkout and a branch
    // switch; the file behind the symbolic ref covers a commit on the branch
    // already checked out. A detached HEAD has no second file and needs none.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    if let Some(reference) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
        println!("cargo:rerun-if-changed=../.git/{reference}");
    }

    let Some(revision) = git(&["rev-parse", REVISION_LENGTH, "HEAD"]) else {
        return UNKNOWN_REVISION.to_string();
    };
    // A tree with uncommitted changes did not come from `revision` alone, and
    // saying so is the whole reason this exists.
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(_) => format!("{revision}-dirty"),
        None => revision,
    }
}

fn main() {
    let source = Path::new("..").join("deploy").join("join.sh");
    println!("cargo:rerun-if-changed={}", source.display());
    let script = std::fs::read_to_string(&source).unwrap_or_default();
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(Path::new(&out_dir).join("join.sh"), script)
        .expect("write the embedded join script");

    // Always set, in every build context, so the crate can read it with
    // `env!` and no consumer needs a fallback of its own.
    println!("cargo:rustc-env={REVISION_OVERRIDE}={}", source_revision());
}

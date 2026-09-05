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
//! A verified release pipeline commit is authoritative. The worker exports
//! `WISENT_SOURCE_COMMIT` from the immutable request and also sets
//! `STADO_SOURCE_REVISION` to that exact value. If both are present they must
//! be the same full lowercase Git commit; inherited parent environment cannot
//! relabel pipeline bytes.
//!
//! Outside the release pipeline, a caller may state the same full commit with
//! `STADO_SOURCE_REVISION`. Otherwise `git rev-parse` names the local checkout,
//! with `-dirty` appended when tracked files differ. A source tree with neither
//! an explicit revision nor Git metadata embeds [`UNKNOWN_REVISION`].
//!
//! One limitation, stated rather than hidden: the rerun triggers below fire on
//! a commit, a checkout and a branch switch, but cargo cannot watch "the whole
//! working tree", so editing a file without touching a watched path does not by
//! itself re-stamp an already-built binary. Any edit that recompiles this crate
//! re-runs the script and re-stamps it.

use std::path::Path;
use std::process::Command;

/// What the revision reads as when no context could name one. The consumer
/// treats this as a value, not an error, so `stado --version` still answers.
const UNKNOWN_REVISION: &str = "unknown";

/// The variable consumed by builds that explicitly state Stado's revision.
const REVISION_OVERRIDE: &str = "STADO_SOURCE_REVISION";

/// The verified source identity exported by Stado's release worker.
const PIPELINE_REVISION: &str = "WISENT_SOURCE_COMMIT";

fn full_git_revision(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Run `git` in the repository root and return trimmed stdout, including an
/// empty string for a successful command with no output. `None` means git is
/// absent, this is not a repository, or the command failed for another reason.
/// Nothing here is allowed to panic or to fail the build.
fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir("..")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

/// The revision this build should claim, by the order documented above.
fn source_revision() -> String {
    println!("cargo:rerun-if-env-changed={REVISION_OVERRIDE}");
    println!("cargo:rerun-if-env-changed={PIPELINE_REVISION}");
    let stated = |name: &str| match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("{name} must be valid Unicode")
        }
    };
    let pipeline = stated(PIPELINE_REVISION);
    let explicit = stated(REVISION_OVERRIDE);
    if let Some(pipeline) = pipeline {
        assert!(
            full_git_revision(&pipeline),
            "{PIPELINE_REVISION} must be a full lowercase Git commit"
        );
        if let Some(explicit) = explicit {
            assert_eq!(
                explicit, pipeline,
                "{REVISION_OVERRIDE} must match authoritative {PIPELINE_REVISION}"
            );
        }
        return pipeline;
    }
    if let Some(explicit) = explicit {
        assert!(
            full_git_revision(&explicit),
            "{REVISION_OVERRIDE} must be a full lowercase Git commit"
        );
        return explicit;
    }

    // Re-stamp when HEAD moves. `.git/HEAD` covers a checkout and a branch
    // switch; the file behind the symbolic ref covers a commit on the branch
    // already checked out. A detached HEAD has no second file and needs none.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    if let Some(reference) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
        println!("cargo:rerun-if-changed=../.git/{reference}");
    }

    let Some(revision) = git(&["rev-parse", "HEAD"]).filter(|value| full_git_revision(value))
    else {
        return UNKNOWN_REVISION.to_string();
    };
    // A tree with uncommitted changes did not come from `revision` alone, and
    // saying so is the whole reason this exists.
    match git(&["status", "--porcelain"]) {
        Some(status) if !status.is_empty() => format!("{revision}-dirty"),
        Some(_) => revision,
        None => UNKNOWN_REVISION.to_string(),
    }
}

fn compile_python(source: &Path, out_dir: &Path) {
    println!("cargo:rerun-if-changed={}", source.display());
    let cache = out_dir.join("python-cache");
    std::fs::create_dir_all(&cache).expect("create the Python compilation cache");
    let output = Command::new("python3")
        .args(["-m", "py_compile"])
        .arg(source)
        .env("PYTHONPYCACHEPREFIX", &cache)
        .output()
        .expect("run the Python compiler for the embedded reconciliation program");
    if !output.status.success() {
        panic!(
            "embedded reconciliation Python did not compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let out_dir = Path::new(&out_dir);
    let source = Path::new("..").join("deploy").join("join.sh");
    println!("cargo:rerun-if-changed={}", source.display());
    let script = std::fs::read_to_string(&source).unwrap_or_default();
    std::fs::write(out_dir.join("join.sh"), script).expect("write the embedded join script");
    compile_python(Path::new("src/deploy/host_storage_reconcile.py"), out_dir);
    // Always set, in every build context, so the crate can read it with
    // `env!` and no consumer needs a fallback of its own.
    println!("cargo:rustc-env={REVISION_OVERRIDE}={}", source_revision());
}

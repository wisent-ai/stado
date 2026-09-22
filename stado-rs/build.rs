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
//! An owner-local recipe supplies the same full base commit while retaining
//! Git metadata. That commit must match the checkout; the embedded revision
//! also reports its measured dirty state. An archive cannot inherit an
//! enclosing checkout's identity.
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

fn checkout_revision() -> Option<String> {
    println!("cargo:rerun-if-changed=../.git/HEAD");
    if let Some(reference) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
        println!("cargo:rerun-if-changed=../.git/{reference}");
    }
    let mut revision = git(&["rev-parse", "HEAD"]).filter(|value| full_git_revision(value))?;
    if !git(&["status", "--porcelain"])?.is_empty() {
        revision.push_str("-dirty");
    }
    Some(revision)
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
        if Path::new("../.git").is_dir() {
            let observed = checkout_revision().expect("read owner-local Git source identity");
            assert_eq!(
                observed.strip_suffix("-dirty").unwrap_or(&observed),
                pipeline,
                "{PIPELINE_REVISION} must match the owner-local checkout"
            );
            return observed;
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

    checkout_revision().unwrap_or_else(|| UNKNOWN_REVISION.to_string())
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
    // The join program a joining machine runs lives as ordinal-prefixed
    // fragments, for the same reason the reconciliation program below does: a
    // single 496-line file could not be edited under this repository's
    // 300-line limit. Sorted order is assembly order and the directory
    // listing is the only list, so adding a fragment needs no edit here. The
    // bytes written are exactly the concatenation, which is what
    // `GET /join.sh` serves and what `fleet ingress up` compares against.
    let join_fragments = Path::new("..").join("deploy").join("join");
    println!("cargo:rerun-if-changed={}", join_fragments.display());
    let mut join_files = std::fs::read_dir(&join_fragments)
        .map(|entries| {
            entries
                .map(|entry| entry.expect("read a join program fragment").path())
                .filter(|path| path.extension().is_some_and(|kind| kind == "sh"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    join_files.sort();
    let script = join_files
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("read a join program fragment"))
        .collect::<String>();
    std::fs::write(out_dir.join("join.sh"), script).expect("write the embedded join script");
    // The embedded reconciliation program lives as contiguous fragments that
    // `deploy::host_storage_reconcile` assembles with
    // `concat!(include_str!(...))`. The directory listing is the only list of
    // fragments: zero-padded ordinal prefixes make sorted order the assembly
    // order, so adding a fragment needs no edit here. Every fragment begins at
    // a top-level statement, so compiling each one alone proves the assembled
    // program parses.
    let fragments = Path::new("src")
        .join("deploy")
        .join("host_storage_reconcile_program");
    println!("cargo:rerun-if-changed={}", fragments.display());
    let mut fragment_files = std::fs::read_dir(&fragments)
        .expect("read the embedded reconciliation program directory")
        .map(|entry| {
            entry
                .expect("read an embedded reconciliation program fragment")
                .path()
        })
        .collect::<Vec<_>>();
    fragment_files.sort();
    assert!(
        !fragment_files.is_empty(),
        "the embedded reconciliation program has no fragments"
    );
    for fragment in &fragment_files {
        compile_python(fragment, out_dir);
    }
    // Always set, in every build context, so the crate can read it with
    // `env!` and no consumer needs a fallback of its own.
    println!("cargo:rustc-env={REVISION_OVERRIDE}={}", source_revision());
}

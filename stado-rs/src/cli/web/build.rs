//! `stado web quality` and `stado web build` — the two steps a web product's
//! `.wisent-release.json` recipe runs on a release worker.
//!
//! Twenty-four landing sites and ten applications would otherwise carry
//! thirty-four copies of the same install-check-build-tar script, and that is
//! exactly what they carried: a `release/` directory per repository, each free
//! to drift. Both steps here read the one worker contract the release pipeline
//! already sets — `WISENT_SOURCE_DIR`, `WISENT_OUTPUT_DIR`, `WISENT_VERSION`,
//! `WISENT_PLATFORM`, `WISENT_INPUTS_DIR` — and refuse by name when one is
//! missing, because a build that guesses where its source or its output lives
//! stages bytes nobody can trace back to a commit.
//!
//! The staged tarball is reproducible: uid and gid 0, no owner names, mtime 0,
//! a fixed entry order, gzip with no timestamp of its own, and each file's
//! mode reduced to 0644 or 0755 by nothing but its execute bit. That is not
//! tidiness. `stado release` publishes an artifact under its sha256 and a
//! unit's `ServiceDeclaration` pins that hash, so a tarball whose bytes drift
//! between two builds of one commit turns every one of those pins into a claim
//! that cannot be checked.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::CmdError;

/// The platform key a web product declares in `.wisent-release.json`. Both
/// steps refuse any other value: the recipe that invoked us is the web one, so
/// a different platform means the manifest names this command under a platform
/// it does not describe, and the artifact it staged would not be runnable by
/// `stado web deploy`.
const PLATFORM: &str = "web";

/// Build outputs of the Next.js compiler that must never enter the artifact.
///
/// `.next/cache` is the webpack and SWC cache: cache packs embed absolute
/// paths from the builder's checkout and their own write times, so two builds
/// of one commit differ inside it. `.next/trace` is the build's own telemetry
/// trace, which is nothing but timestamps. Neither is read at runtime — Next
/// documents `.next/cache` as build-only state — so excluding them costs the
/// unit nothing and is what makes the tarball reproducible at all.
const EXCLUDED_BUILD_OUTPUT: [&str; 2] = ["cache", "trace"];

/// The release worker's contract, as read from the environment once.
struct Worker {
    /// The checkout the worker prepared. Every command runs inside it.
    source: PathBuf,
    /// Where staged files go; the release pipeline collects them from here.
    output: PathBuf,
    /// The version the pipeline is cutting, which `package.json` must agree with.
    version: String,
    /// The platform key the recipe declared this step under.
    platform: String,
    /// Artifacts of earlier platforms, staged for this one to consume. A web
    /// product consumes none, so this may be empty; it is reported so the
    /// release log says whether anything was handed in.
    inputs: String,
}

/// One variable of the worker contract, refused by name.
///
/// The shell version of this gate read `: "${WISENT_SOURCE_DIR:?Stado must
/// provide WISENT_SOURCE_DIR}"`, and the wording is kept: the operator reading
/// a failed release log needs to know the variable is Stado's to supply, not
/// something they forgot to export.
fn required(name: &str) -> Result<String, CmdError> {
    let value = std::env::var(name).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Err(CmdError::click(format!(
            "{name} is not set: Stado must provide it to a release step, so this step is running outside the release worker"
        )));
    }
    Ok(value.to_string())
}

fn worker() -> Result<Worker, CmdError> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    let output = PathBuf::from(required("WISENT_OUTPUT_DIR")?);
    let version = required("WISENT_VERSION")?;
    let platform = required("WISENT_PLATFORM")?;
    // Read but not required: a web product declares no inputs, and refusing an
    // empty value would fail every web release for a variable nothing reads.
    let inputs = std::env::var("WISENT_INPUTS_DIR").unwrap_or_default();
    if !source.is_dir() {
        return Err(CmdError::click(format!(
            "WISENT_SOURCE_DIR names {}, which is not a directory: the release worker did not prepare a checkout there",
            source.display()
        )));
    }
    Ok(Worker {
        source,
        output,
        version,
        platform,
        inputs: inputs.trim().to_string(),
    })
}

impl Worker {
    /// Refuse a platform this recipe does not describe.
    fn require_web_platform(&self) -> Result<(), CmdError> {
        if self.platform != PLATFORM {
            return Err(CmdError::click(format!(
                "WISENT_PLATFORM is `{}` but this is the `{PLATFORM}` recipe: the product's .wisent-release.json calls `stado web` under a platform that is not a web platform",
                self.platform
            )));
        }
        Ok(())
    }

    /// What the release log should say about handed-in artifacts.
    fn inputs_report(&self) -> String {
        if self.inputs.is_empty() {
            "no release inputs were staged for this platform".to_string()
        } else {
            format!("release inputs at {}", self.inputs)
        }
    }
}

/// `package.json` as an object, with every way it can be unusable named.
fn manifest(source: &Path) -> Result<Map<String, Value>, CmdError> {
    let path = source.join("package.json");
    let bytes = std::fs::read(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot read {}: {error}. A web product is a Node package; without its package.json there is no build script, no start script and no version to check",
            path.display()
        ))
    })?;
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!("{} is not valid JSON: {error}", path.display()))
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(CmdError::click(format!(
            "{} is not a JSON object",
            path.display()
        ))),
    }
}

/// The version in `package.json`, refused when it disagrees with the version
/// the pipeline is cutting.
///
/// A web product's `version_source` points at this same field, so a mismatch
/// means the worker is building a checkout other than the one the release was
/// submitted for — and the artifact would be published under a version its own
/// `package.json` denies.
fn require_version(manifest: &Map<String, Value>, version: &str) -> Result<(), CmdError> {
    match manifest.get("version").and_then(Value::as_str) {
        Some(declared) if declared == version => Ok(()),
        Some(declared) => Err(CmdError::click(format!(
            "package.json declares version {declared} but WISENT_VERSION is {version}: the worker is not building the commit this release was cut from"
        ))),
        None => Err(CmdError::click(
            "package.json declares no version: the release pipeline reads the product's version from that field",
        )),
    }
}

/// One npm script, if the product declares a non-empty one under that name.
fn script<'a>(manifest: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    manifest
        .get("scripts")?
        .as_object()?
        .get(name)?
        .as_str()
        .filter(|body| !body.trim().is_empty())
}

/// The artifact's product name: `package.json`'s `name` with any `@scope/`
/// prefix removed.
///
/// The scope cannot survive into a file name — `dist/@wisent/foo-web.tar.gz`
/// is a second directory the recipe's stage map does not name, so the file
/// would never be collected. Anything else with a path separator in it is
/// refused rather than sanitised, because a staged path is what the manifest
/// matches on and quietly rewriting it produces an artifact the recipe cannot
/// find.
fn product_name(package_name: &str) -> Result<&str, CmdError> {
    let bare = match package_name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, name)) if !scope.is_empty() => name,
            _ => {
                return Err(CmdError::click(format!(
                    "package.json name `{package_name}` starts with @ but names no scope: expected `@scope/name`"
                )))
            }
        },
        None => package_name,
    };
    if bare.is_empty() {
        return Err(CmdError::click(
            "package.json declares an empty name: the staged artifact is named after it",
        ));
    }
    if bare.contains('/') || bare.contains('\\') || bare == "." || bare == ".." {
        return Err(CmdError::click(format!(
            "package.json name `{package_name}` is not usable as a file name: the staged artifact is named after it"
        )));
    }
    Ok(bare)
}

/// The product name of the checkout being built.
///
/// `.wisent-release.json`'s `product` first, and `package.json`'s `name` only
/// when the checkout carries no manifest. The two disagree in practice and the
/// manifest is the one that matters: `preferences-landing`'s `package.json` is
/// named `preferences`, so naming the artifact after the package staged
/// `dist/preferences-web.tar.gz` while the recipe's stage map named
/// `dist/preferences-landing-web.tar.gz` — a build that succeeds and collects
/// nothing, which is the worst shape a release step can have. The stage map
/// and this name are two statements about one file, so both are read from the
/// document the release pipeline itself parses.
fn product(source: &Path, manifest: &Map<String, Value>) -> Result<String, CmdError> {
    let release_manifest = source.join(crate::release_pipeline::PRODUCT_MANIFEST);
    if let Ok(text) = std::fs::read_to_string(&release_manifest) {
        let declared: Value = serde_json::from_str(&text).map_err(|error| {
            CmdError::click(format!(
                "{} is not valid JSON: {error}",
                release_manifest.display()
            ))
        })?;
        if let Some(name) = declared.get("product").and_then(Value::as_str) {
            return product_name(name).map(str::to_string);
        }
        return Err(CmdError::click(format!(
            "{} declares no product, and the staged artifact is named after it",
            release_manifest.display()
        )));
    }
    let declared = manifest
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(
            "the checkout carries no .wisent-release.json and package.json declares no name, so \
             the staged artifact has nothing to be named after",
        )
        })?;
    product_name(declared).map(str::to_string)
}

/// The archive's single top-level directory, so an extraction cannot scatter
/// files into whatever directory it was run from.
fn top_level(product: &str, version: &str) -> String {
    format!("{product}-{version}")
}

/// The staged tarball's file name, which the recipe's `stage` map names
/// verbatim. One function so the sidecar and the path can never disagree.
fn tarball_name(product: &str) -> String {
    format!("{product}-web.tar.gz")
}

/// The `sha256sum -c`-readable sidecar line: digest, two spaces, file name.
fn sidecar_line(digest: &str, file_name: &str) -> String {
    format!("{digest}  {file_name}\n")
}

/// How a failed command's exit is reported. A killed process has no code, and
/// "exit status 0" would be a lie about a build that died on SIGKILL because
/// the builder ran out of memory — which is how a Next.js build fails.
fn exit_report(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => status.to_string(),
    }
}

/// Run one command in the checkout with its stdout and stderr inherited, so
/// the release log carries the tool's own diagnostics rather than a Stado
/// paraphrase of them. stdin is closed: a builder has no operator to answer a
/// prompt, and a release that hangs on one holds the worker forever.
///
/// `path` replaces the inherited `PATH`, and exists for one reason: a program
/// that is itself a script needs its interpreter findable, and the only caller
/// that knows where that interpreter is, is the one that just resolved the
/// script.
fn run_with_path(
    source: &Path,
    program: &str,
    arguments: &[&str],
    toolchain: &str,
    path: Option<&str>,
) -> Result<(), CmdError> {
    let rendered = format!("{program} {}", arguments.join(" "));
    println!("stado web: {rendered}");
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(source)
        .stdin(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CmdError::click(format!(
                "`{program}` is not there: the builder running the `{PLATFORM}` platform has no \
                 {toolchain}, so give that platform a `runner_platform` whose host carries one"
            ))
        } else {
            CmdError::click(format!("cannot run `{rendered}`: {error}"))
        }
    })?;
    if !status.success() {
        return Err(CmdError::click(format!(
            "`{rendered}` failed with {}",
            exit_report(status)
        )));
    }
    Ok(())
}

/// Where a fleet host installs the Node toolchain, in probe order.
///
/// The same order `host_exec`'s candidate table probes, so a release step and
/// a `stado host exec node --version` on the same host cannot name different
/// binaries. The list exists because a release worker is a launchd job, and a
/// launchd job's `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin` unless something
/// set it — Homebrew is not on it. `charless-mac-mini` carries node v25.9.0
/// and npm 11.12.1 under `/opt/homebrew/bin`, and a build that trusted `PATH`
/// would have reported that host as having no Node toolchain at all.
const NODE_TOOLCHAIN_DIRECTORIES: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

/// The absolute path of one Node toolchain program, or its bare name.
///
/// Falling back to the bare name rather than refusing keeps a builder that
/// installs Node somewhere else working: `PATH` is then the answer, and
/// [`run`]'s own refusal names the toolchain if that fails too.
fn toolchain_program(name: &str) -> String {
    NODE_TOOLCHAIN_DIRECTORIES
        .iter()
        .map(|directory| PathBuf::from(directory).join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

/// Run one `npm` verb with the interpreter its shim needs.
///
/// `npm` is a JavaScript shim whose first line is `#!/usr/bin/env node`, so
/// running it with a `PATH` that does not carry `node` fails with
/// `env: node: No such file or directory` and says nothing about npm. The
/// directory the resolved `npm` came from is prepended to `PATH`, because the
/// interpreter a shim needs is always its sibling.
fn npm(source: &Path, arguments: &[&str]) -> Result<(), CmdError> {
    let program = toolchain_program("npm");
    let mut path = std::env::var("PATH").unwrap_or_default();
    if let Some(directory) = PathBuf::from(&program).parent() {
        let directory = directory.to_string_lossy().into_owned();
        if !directory.is_empty() && !directory.starts_with("npm") {
            path = if path.is_empty() {
                directory
            } else {
                format!("{directory}:{path}")
            };
        }
    }
    run_with_path(source, &program, arguments, "Node toolchain", Some(&path))
}

/// Install exactly the tree the lockfile pins.
///
/// `npm install` is deliberately not a fallback. It resolves versions afresh,
/// so the artifact would carry whatever was newest on the registry the minute
/// the builder ran, and two builds of one commit would ship different
/// dependencies. An unlocked install is not a release input.
fn install(source: &Path) -> Result<(), CmdError> {
    let lockfile = source.join("package-lock.json");
    if !lockfile.is_file() {
        return Err(CmdError::click(format!(
            "{} is missing: a web release installs only what a lockfile pins, and Stado will not fall back to `npm install` because that resolves versions at build time and makes the artifact unreproducible",
            lockfile.display()
        )));
    }
    npm(source, &["ci", "--no-audit", "--no-fund"])
}

/// The commit the artifact was cut from, recorded beside it.
fn revision(source: &Path) -> Result<String, CmdError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CmdError::click(
                    "`git` is not on PATH: the builder cannot record which commit the artifact was cut from",
                )
            } else {
                CmdError::click(format!("cannot run `git rev-parse HEAD`: {error}"))
            }
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "`git rev-parse HEAD` in {} failed with {}: {}",
            source.display(),
            exit_report(output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if revision.is_empty() {
        return Err(CmdError::click(format!(
            "`git rev-parse HEAD` in {} printed nothing",
            source.display()
        )));
    }
    Ok(revision)
}

/// The launcher the managed unit executes, generated rather than carried by
/// the product so all thirty-four web products start the same way.
///
/// It resolves its own root from `$0` because the tarball is extracted under
/// `$HOME/.stado/services/<name>/current` on whichever host runs the unit, and
/// a path baked in at build time would be the builder's path, not that one.
/// It sources `WEB_ENV_FILE` because that is where `stado service secret-sync`
/// puts a Skarbiec value: an owner-only file, never the unit document, since a
/// secret written into a launchd plist or a systemd unit is a secret committed
/// to the registry. Nothing else in the run path would read it, and a secret
/// delivered but never read is an outage that reports success.
fn launcher() -> &'static str {
    // The script carries its own reasoning, because whoever reads it next will
    // be reading it inside an install root on a fleet host, with none of this
    // module in front of them.
    r#"#!/bin/sh
# Generated by `stado web build`. Starts this product's own `start` script on
# the loopback port the managed unit was declared with.
#
# --hostname 127.0.0.1 is load-bearing, not a default. The public edge is what
# holds 443 and terminates TLS for this product's hostname; a unit bound to
# every interface would be a second entrance to the same application, reachable
# across the tailnet with nothing in front of it and no certificate involved.
# Loopback only, so the edge is the only way in.
set -eu

# The install root is wherever the release was extracted, so it is resolved
# from $0 rather than written in at build time.
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# The port is checked, and captured, before anything else can define one. The
# unit passes the port this product was declared with; a value sourced from the
# environment file must not be able to answer for it, or a product could quietly
# listen somewhere the edge does not forward to.
if [ -z "${PORT:-}" ]; then
  echo "start-web: PORT is unset; the managed unit must pass the loopback port this product was declared with" >&2
  exit 1
fi
port=$PORT

# The unit's secrets arrive in an owner-only file whose absolute path the unit
# passes as WEB_ENV_FILE, written one variable at a time by
# `stado service secret-sync`. `set -a` is what exports the assignments into
# the server's environment; a plain `.` would leave them as shell variables the
# server never sees, and every delivered credential would be dead text.
if [ -n "${WEB_ENV_FILE:-}" ] && [ -f "$WEB_ENV_FILE" ]; then
  set -a
  . "$WEB_ENV_FILE"
  set +a
fi

exec npm run start -- --port "$port" --hostname 127.0.0.1
"#
}

pub(crate) fn quality() -> Result<(), CmdError> {
    let worker = worker()?;
    worker.require_web_platform()?;
    let manifest = manifest(&worker.source)?;
    require_version(&manifest, &worker.version)?;
    let product = product(&worker.source, &manifest)?;
    println!(
        "stado web quality: {product} {} in {} ({})",
        worker.version,
        worker.source.display(),
        worker.inputs_report()
    );

    // Both scripts are checked before anything is installed, because an
    // install of a tree Stado could never run afterwards wastes the whole gate.
    // `build` is what produces `.next`; `start` is what the generated launcher
    // executes on the unit's port. A product missing either is not something
    // `stado web deploy` can host, and saying so here is cheaper than saying it
    // after a release has been published.
    let mut missing = Vec::new();
    if script(&manifest, "build").is_none() {
        missing.push("build");
    }
    if script(&manifest, "start").is_none() {
        missing.push("start");
    }
    if !missing.is_empty() {
        return Err(CmdError::click(format!(
            "package.json declares no {} script: a web product Stado hosts needs both `build`, which produces .next, and `start`, which the generated {} launcher runs on the unit's port",
            missing.join(" and no "),
            super::LAUNCHER
        )));
    }

    install(&worker.source)?;

    // The product's own checks, not Stado's opinion of them. A landing site
    // with neither script is a legitimate web product; it just has nothing
    // here to run, and the log says so rather than leaving the operator to
    // wonder which check passed.
    let mut ran = Vec::new();
    for check in ["typecheck", "lint"] {
        if script(&manifest, check).is_some() {
            npm(&worker.source, &["run", check])?;
            ran.push(check);
        }
    }
    if ran.is_empty() {
        println!(
            "stado web quality: {product} declares neither a typecheck nor a lint script; the locked install is the whole gate"
        );
    } else {
        println!("stado web quality: {product} passed {}", ran.join(" and "));
    }
    Ok(())
}

pub(crate) fn build() -> Result<(), CmdError> {
    let worker = worker()?;
    worker.require_web_platform()?;
    let manifest = manifest(&worker.source)?;
    require_version(&manifest, &worker.version)?;
    let product = product(&worker.source, &manifest)?;
    println!(
        "stado web build: {product} {} in {} ({})",
        worker.version,
        worker.source.display(),
        worker.inputs_report()
    );

    // The worker may run the build in a checkout that never saw the quality
    // step — a re-run of one platform, or a recipe with no quality gate — so
    // the install is repeated when, and only when, there is no tree to build
    // against.
    if worker.source.join("node_modules").is_dir() {
        println!("stado web build: node_modules is present from the quality step");
    } else {
        install(&worker.source)?;
    }

    npm(&worker.source, &["run", "build"])?;

    let revision = revision(&worker.source)?;
    let dist = worker.output.join("dist");
    std::fs::create_dir_all(&dist)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", dist.display())))?;
    let file_name = tarball_name(&product);
    let tarball = dist.join(&file_name);
    let root = top_level(&product, &worker.version);
    stage(&worker.source, &tarball, &root)?;

    // The digest is streamed rather than taken over the whole file in memory:
    // a tarball carrying node_modules runs to hundreds of megabytes, and the
    // builder is a fleet host with other work on it.
    let digest = digest(&tarball)?;
    let sidecar = dist.join(format!("{file_name}.sha256"));
    std::fs::write(&sidecar, sidecar_line(&digest, &file_name))
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", sidecar.display())))?;
    let source_revision = dist.join("SOURCE_REVISION");
    std::fs::write(&source_revision, format!("{revision}\n")).map_err(|error| {
        CmdError::click(format!(
            "cannot write {}: {error}",
            source_revision.display()
        ))
    })?;

    let bytes = std::fs::metadata(&tarball)?.len();
    println!(
        "stado web build: staged {} ({bytes} bytes, sha256 {digest}) from {revision}",
        tarball.display()
    );
    Ok(())
}

/// Every path that goes into the artifact, in the one order two builds of a
/// commit will always produce: a fixed root order, then each directory's
/// entries sorted by name.
fn members(source: &Path) -> Result<Vec<PathBuf>, CmdError> {
    // Required members, each refused with what its absence means. A tarball
    // missing any of them installs as a unit that cannot start.
    let required: [(&str, &str); 4] = [
        (
            "package.json",
            "the unit runs the product's own `start` script through it",
        ),
        (
            "package-lock.json",
            "the artifact records the tree it was installed from",
        ),
        (
            ".next",
            "`npm run build` produced no build output, so the product's build script did not build a Next.js application",
        ),
        (
            "node_modules",
            "the unit runs from the artifact and installs nothing at deploy time",
        ),
    ];
    for (name, why) in required {
        if !source.join(name).exists() {
            return Err(CmdError::click(format!(
                "{} is missing after the build: {why}",
                source.join(name).display()
            )));
        }
    }

    let mut roots = vec![
        source.join("package.json"),
        source.join("package-lock.json"),
        source.join(".next"),
    ];
    if source.join("public").is_dir() {
        roots.push(source.join("public"));
    }
    // `next.config.ts`, `.js`, `.mjs` — the extension is the product's choice,
    // and the runtime reads whichever one it finds, so all of them travel.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(source)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", source.display())))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("next.config."))
        })
        .collect();
    entries.sort();
    roots.extend(entries);
    roots.push(source.join("node_modules"));

    let excluded: Vec<PathBuf> = EXCLUDED_BUILD_OUTPUT
        .iter()
        .map(|name| source.join(".next").join(name))
        .collect();
    let mut members = Vec::new();
    for root in roots {
        members.push(root.clone());
        if std::fs::symlink_metadata(&root)?.is_dir() {
            walk(&root, &excluded, &mut members)?;
        }
    }
    Ok(members)
}

/// One directory's contents, name-sorted, then each subdirectory's.
///
/// The recursion tests the entry with `symlink_metadata`, so a link to a
/// directory is recorded and not descended into. `node_modules` can hold a
/// link back into itself, and descending one is how a walk of it never ends.
fn walk(directory: &Path, excluded: &[PathBuf], into: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", directory.display())))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for entry in entries {
        if excluded.contains(&entry) {
            continue;
        }
        into.push(entry.clone());
        if std::fs::symlink_metadata(&entry)?.is_dir() {
            walk(&entry, excluded, into)?;
        }
    }
    Ok(())
}

/// A file's mode as the artifact records it: nothing of the builder's umask
/// survives, only whether the file is executable.
#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o100 == 0o100 {
        0o755
    } else {
        0o644
    }
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0o644
}

/// A header with everything about the builder erased: no owner, no owner name,
/// no modification time. Two builders with different user ids produce the same
/// bytes.
fn header(entry_type: tar::EntryType, mode: u32, size: u64) -> Result<tar::Header, CmdError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("")?;
    header.set_groupname("")?;
    Ok(header)
}

/// Write the runnable tarball: one top-level directory, the build output, the
/// installed tree, and the generated launcher.
fn stage(source: &Path, tarball: &Path, root: &str) -> Result<(), CmdError> {
    let members = members(source)?;
    let file = std::fs::File::create(tarball).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", tarball.display()))
    })?;
    // gzip's own header carries a modification time, and it is set to zero for
    // the same reason every entry's is: the artifact's bytes must depend on the
    // commit and nothing else. No file name is stored either, since that would
    // record the builder's path.
    let encoder = flate2::GzBuilder::new().mtime(0).write(
        std::io::BufWriter::new(file),
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);

    let prefix = Path::new(root);
    archive.append_data(
        &mut header(tar::EntryType::Directory, 0o755, 0)?,
        prefix,
        std::io::empty(),
    )?;
    for member in &members {
        let relative = member.strip_prefix(source).map_err(|_| {
            CmdError::click(format!(
                "{} is not inside {}",
                member.display(),
                source.display()
            ))
        })?;
        let name = prefix.join(relative);
        let metadata = std::fs::symlink_metadata(member)?;
        let kind = metadata.file_type();
        if kind.is_symlink() {
            // A symlink travels as a symlink and is never followed.
            // `node_modules/.bin` is a directory of them, and the scripts they
            // point at resolve their own `require` paths relative to where the
            // link's target lives, so a dereferenced copy would start and then
            // fail to find its own package. A link can also point back into
            // node_modules, which is how following one walks forever. The mode
            // is fixed rather than copied because no extractor applies a
            // symlink's mode, and lstat reports a different one on Darwin than
            // on Linux -- which would be enough to make the same commit build
            // to two different tarballs on two builders.
            let target = std::fs::read_link(member)?;
            let mut entry = header(tar::EntryType::Symlink, 0o777, 0)?;
            archive.append_link(&mut entry, &name, &target)?;
        } else if kind.is_dir() {
            archive.append_data(
                &mut header(tar::EntryType::Directory, 0o755, 0)?,
                &name,
                std::io::empty(),
            )?;
        } else if kind.is_file() {
            let mut handle = std::fs::File::open(member).map_err(|error| {
                CmdError::click(format!("cannot read {}: {error}", member.display()))
            })?;
            let mut entry = header(
                tar::EntryType::Regular,
                file_mode(&metadata),
                metadata.len(),
            )?;
            archive.append_data(&mut entry, &name, &mut handle)?;
        } else {
            // A socket or a fifo in the tree is not something tar can carry
            // faithfully, and silently dropping it would produce an artifact
            // whose contents nobody declared.
            return Err(CmdError::click(format!(
                "{} is neither a file, a directory nor a symlink and cannot be staged",
                member.display()
            )));
        }
    }

    let script = launcher();
    archive.append_data(
        &mut header(tar::EntryType::Directory, 0o755, 0)?,
        prefix.join("bin"),
        std::io::empty(),
    )?;
    let mut entry = header(tar::EntryType::Regular, 0o755, script.len() as u64)?;
    archive.append_data(&mut entry, prefix.join(super::LAUNCHER), script.as_bytes())?;

    let mut writer = archive.into_inner()?.finish()?;
    writer.flush()?;
    Ok(())
}

/// The artifact's sha256, read back from the file that was just written so the
/// digest describes the bytes on disk rather than the bytes we meant to write.
fn digest(tarball: &Path) -> Result<String, CmdError> {
    let mut file = std::fs::File::open(tarball)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", tarball.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_package_name_is_the_product_name() {
        assert_eq!(
            product_name("preferences-landing").unwrap(),
            "preferences-landing"
        );
    }

    #[test]
    fn scope_is_stripped_from_a_scoped_package_name() {
        assert_eq!(product_name("@wisent/preferences").unwrap(), "preferences");
    }

    #[test]
    fn a_name_that_is_not_a_file_name_is_refused() {
        // A scope left in would put the artifact in a directory the recipe's
        // stage map never names, so it would never be collected.
        for name in ["", "@wisent", "@/preferences", "web/preferences", ".."] {
            let error = product_name(name).expect_err(name);
            assert!(
                error.message.is_some_and(|message| !message.is_empty()),
                "refusing {name} must say why"
            );
        }
    }

    #[test]
    fn archive_paths_are_named_after_the_product_and_version() {
        assert_eq!(top_level("preferences", "1.4.0"), "preferences-1.4.0");
        assert_eq!(tarball_name("preferences"), "preferences-web.tar.gz");
    }

    #[test]
    fn the_sidecar_line_is_sha256sum_readable() {
        let digest = "e".repeat(64);
        assert_eq!(
            sidecar_line(&digest, "preferences-web.tar.gz"),
            format!("{digest}  preferences-web.tar.gz\n")
        );
    }

    #[test]
    fn the_launcher_binds_loopback_and_needs_a_port() {
        let script = launcher();
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.ends_with("exec npm run start -- --port \"$port\" --hostname 127.0.0.1\n"));
        // The reason loopback is not negotiable travels with the script.
        assert!(script.contains("terminates TLS"));
        // PORT unset is a refusal on stderr, not a default port.
        assert!(script.contains("if [ -z \"${PORT:-}\" ]; then"));
        assert!(script.contains(">&2"));
        assert!(script.contains("exit 1"));
        // The install root is resolved at run time; no builder path survives.
        assert!(script.contains("cd -- \"$(dirname -- \"$0\")/..\""));
        assert!(!script.contains("/Users/"));
        assert!(!script.contains("/home/"));
    }

    #[test]
    fn the_launcher_exports_the_units_secret_file() {
        // `stado service secret-sync` writes VAR=value into an owner-only file
        // rather than into the unit document. If the launcher does not source
        // it, and export what it sources, every credential the fleet delivers
        // to a web unit is unset at runtime while the deploy reports success.
        let script = launcher();
        assert!(
            script.contains("if [ -n \"${WEB_ENV_FILE:-}\" ] && [ -f \"$WEB_ENV_FILE\" ]; then")
        );
        let exporting = script
            .find("set -a\n  . \"$WEB_ENV_FILE\"\n  set +a")
            .expect("the environment file must be sourced with assignments exported");
        // The declared port is read, and captured, before the file can define
        // one: a sourced PORT must not be able to answer for the port the unit
        // was declared with, or the product would listen where the edge does
        // not forward.
        let port_check = script
            .find("if [ -z \"${PORT:-}\" ]; then")
            .expect("the launcher must refuse an unset PORT");
        let capture = script
            .find("port=$PORT")
            .expect("the launcher must capture the declared port");
        assert!(port_check < capture && capture < exporting);
    }

    #[test]
    fn only_a_declared_non_empty_script_counts() {
        let manifest = serde_json::json!({
            "scripts": { "build": "next build", "lint": "   " }
        });
        let manifest = manifest.as_object().unwrap();
        assert_eq!(script(manifest, "build"), Some("next build"));
        // A script declared as whitespace runs nothing; treating it as present
        // would have the gate report a lint that never happened.
        assert_eq!(script(manifest, "lint"), None);
        assert_eq!(script(manifest, "typecheck"), None);
        assert_eq!(script(&Map::new(), "build"), None);
    }

    #[test]
    fn the_version_must_match_the_release_being_cut() {
        let matching = serde_json::json!({ "version": "1.4.0" });
        require_version(matching.as_object().unwrap(), "1.4.0").unwrap();

        let drifted = serde_json::json!({ "version": "1.3.9" });
        let error = require_version(drifted.as_object().unwrap(), "1.4.0")
            .expect_err("a version mismatch must be refused");
        let message = error.message.unwrap_or_default();
        assert!(
            message.contains("1.3.9") && message.contains("1.4.0"),
            "{message}"
        );

        let absent = serde_json::json!({});
        require_version(absent.as_object().unwrap(), "1.4.0")
            .expect_err("a package.json with no version must be refused");
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_command_is_not_reported_as_a_clean_exit() {
        use std::os::unix::process::ExitStatusExt;

        // A wait status of 7 << 8 is an exit code of 7.
        assert_eq!(exit_report(ExitStatus::from_raw(7 << 8)), "exit status 7");
        // A Next.js build that exhausts the builder's memory is killed and has
        // no exit code at all. Reporting a code there would say the build
        // returned something, and the operator would look for a compile error
        // that was never printed.
        let killed = exit_report(ExitStatus::from_raw(9));
        assert!(killed.contains("signal"), "{killed}");
        assert!(!killed.contains("exit status"), "{killed}");
    }
}

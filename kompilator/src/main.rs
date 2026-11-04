//! `kompilator`: builds a Rust package from its `Cargo.lock` by running
//! `rustc` directly, without Cargo. It reads the locked graph, resolves
//! every package's features for the target and the host, runs build
//! scripts, compiles procedural macros and libraries in parallel in
//! dependency order, starting each library as soon as the interfaces under
//! it are written, and links the package's binaries. A unit whose inputs did
//! not change is reused from the cache.
//!
//! It depends on nothing but Rust's standard library, so it is built with
//! `rustc` alone: `rustc --edition 2021 -O src/main.rs -o kompilator`.

mod binaries;
mod cfg;
mod compile;
mod lock;
mod manifest;
mod profile;
mod resolve;
mod script;
mod revision;
mod sha256;
mod toml;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "kompilator build --manifest-path <dir>/Cargo.toml [--revision REV] [--release] [--bin NAME]... [--features A,B] [--no-default-features] [--all-features] [--out DIR] [--cache DIR] [--jobs N] [--incremental]
  Build the package whose manifest is named, from the Cargo.lock beside it, with rustc and no Cargo.
  Prints the path of each binary it linked on standard output, one per line.
  --revision REV          build this commit of the manifest's repository, exported once, instead of the working tree
  --release               the release profile; the dev profile without it. [profile.*] of the manifest applies
  --bin NAME              link only this binary of the package; every binary when none is named
  --features A,B          enable these features of the package (repeat or separate with commas or spaces)
  --no-default-features   leave the package's default feature off
  --all-features          enable every feature the package declares
  --out DIR               where the binaries go; <package dir>/.build/kompilator/<profile> by default
  --cache DIR             where compiled units are kept; KOMPILATOR_CACHE, else the --out directory.
                          Products built with one cache share every unit they have in common
  --jobs N                how many units compile at once; the machine's cores by default
  --incremental           compile the package's own sources incrementally whatever the profile says
kompilator where --manifest-path <dir>/Cargo.toml [--release] [--bin NAME]... [--out DIR]
  Print the path of each binary a build with the same flags writes, one per line, without building;
  a binary that is not there is refused with the build command that writes it.";

/// What the invocation asks for.
#[derive(PartialEq)]
enum Verb {
    Build,
    Where,
}

struct Request {
    verb: Verb,
    manifest: PathBuf,
    release: bool,
    bins: Vec<String>,
    out: Option<PathBuf>,
    cache: Option<PathBuf>,
    jobs: Option<usize>,
    incremental: bool,
    features: resolve::Features,
    revision: Option<String>,
}

fn request(arguments: &[String]) -> Result<Request, String> {
    let mut words = arguments.iter();
    let verb = match words.next().map(String::as_str) {
        Some("build") => Verb::Build,
        Some("where") => Verb::Where,
        Some("--help") | Some("-h") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("kompilator takes build or where; got {other}\n{USAGE}")),
    };
    let name = match verb {
        Verb::Build => "build",
        Verb::Where => "where",
    };
    let mut request = Request {
        verb,
        manifest: PathBuf::new(),
        release: false,
        bins: Vec::new(),
        out: None,
        cache: None,
        jobs: None,
        incremental: false,
        features: resolve::Features { default: true, all: false, named: Vec::new() },
        revision: None,
    };
    let mut manifest = None;
    while let Some(word) = words.next() {
        let mut value = |flag: &str| words.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match word.as_str() {
            // `where` reads only what decides the output paths; a flag that
            // changes what is compiled, not where, is refused for it.
            flag if request.verb == Verb::Where
                && !matches!(flag, "--manifest-path" | "--release" | "--bin" | "--out") =>
            {
                return Err(format!(
                    "kompilator where does not take {flag}: it reads only --manifest-path, --release, --bin and --out, which decide where the binaries are\n{USAGE}"
                ))
            }
            "--manifest-path" => manifest = Some(PathBuf::from(value("--manifest-path")?)),
            "--revision" => request.revision = Some(value("--revision")?),
            "--release" => request.release = true,
            "--incremental" => request.incremental = true,
            "--no-default-features" => request.features.default = false,
            "--all-features" => request.features.all = true,
            "--features" => {
                let given = value("--features")?;
                request.features.named.extend(
                    given.split([',', ' ']).map(str::trim).filter(|name| !name.is_empty()).map(String::from),
                );
            }
            "--bin" => request.bins.push(value("--bin")?),
            "--out" => request.out = Some(PathBuf::from(value("--out")?)),
            "--cache" => request.cache = Some(PathBuf::from(value("--cache")?)),
            "--jobs" => {
                let given = value("--jobs")?;
                let jobs: usize = given.parse().map_err(|_| format!("--jobs takes a positive whole number, got {given}"))?;
                if jobs == 0 {
                    return Err(String::from("--jobs takes a positive whole number, got 0"));
                }
                request.jobs = Some(jobs);
            }
            other => return Err(format!("kompilator {name} does not take {other}\n{USAGE}")),
        }
    }
    request.manifest = manifest.ok_or_else(|| format!("kompilator {name} needs --manifest-path\n{USAGE}"))?;
    Ok(request)
}

/// Hold `dir`'s lock for the build, so two builds never write one unit at
/// once; a second build waits, saying so.
fn hold(dir: &Path) -> Result<File, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{} cannot be made: {error}", dir.display()))?;
    let path = dir.join(".lock");
    let file = File::create(&path).map_err(|error| format!("the lock {} cannot be opened: {error}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!("kompilator: another build holds {}; waiting for it", path.display());
            file.lock().map_err(|error| format!("the lock {} cannot be taken: {error}", path.display()))?;
            Ok(file)
        }
        Err(std::fs::TryLockError::Error(error)) => Err(format!("the lock {} cannot be taken: {error}", path.display())),
    }
}

/// The compiler the package is built with: RUSTC when set, else the
/// `rustc` the toolchain manager chooses in the package's own directory
/// (its `rust-toolchain.toml`), named by its full path so every unit and
/// build script uses that same compiler wherever it runs.
fn compiler(root_dir: &Path) -> Result<String, String> {
    if let Some(rustc) = std::env::var("RUSTC").ok().filter(|rustc| !rustc.is_empty()) {
        return Ok(rustc);
    }
    let printed = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(root_dir)
        .output()
        .map_err(|error| {
            format!("rustc could not run in {}: {error}; install Rust or name the compiler with RUSTC", root_dir.display())
        })?;
    if !printed.status.success() {
        return Err(format!(
            "rustc --print sysroot in {} exited {}: {}",
            root_dir.display(),
            printed.status,
            String::from_utf8_lossy(&printed.stderr).trim()
        ));
    }
    let sysroot = PathBuf::from(String::from_utf8_lossy(&printed.stdout).trim());
    let rustc = sysroot.join("bin").join("rustc");
    if !rustc.exists() {
        return Err(format!("the toolchain at {} holds no bin/rustc", sysroot.display()));
    }
    Ok(rustc.display().to_string())
}

/// The manifest's directory, refusing a manifest that cannot be found.
fn checkout_of(request: &Request) -> Result<PathBuf, String> {
    let manifest_path = request
        .manifest
        .canonicalize()
        .map_err(|error| format!("the manifest {} cannot be found: {error}", request.manifest.display()))?;
    Ok(manifest_path.parent().ok_or("the manifest has no directory")?.to_path_buf())
}

/// The `kompilator build` command that writes what `request` locates.
fn build_command(request: &Request, checkout_dir: &Path) -> String {
    let mut words = vec![
        String::from("kompilator build --manifest-path"),
        checkout_dir.join("Cargo.toml").display().to_string(),
    ];
    if request.release {
        words.push(String::from("--release"));
    }
    for bin in &request.bins {
        words.push(format!("--bin {bin}"));
    }
    if let Some(out) = &request.out {
        words.push(format!("--out {}", out.display()));
    }
    words.join(" ")
}

/// `kompilator where`: each selected binary's path in the build's output,
/// one per line, without building.
fn locate(request: &Request) -> Result<(), String> {
    let checkout_dir = checkout_of(request)?;
    let manifest = manifest::Manifest::read(&checkout_dir)?;
    let bins = binaries::wanted(&manifest, &request.bins)?;
    let out = binaries::out_dir(&checkout_dir, request.release, request.out.as_deref());
    for path in binaries::located(&bins, &out, &build_command(request, &checkout_dir))? {
        println!("{}", path.display());
    }
    Ok(())
}

fn build(request: &Request) -> Result<(), String> {
    let started = Instant::now();
    let checkout_dir = checkout_of(request)?;
    let out = binaries::out_dir(&checkout_dir, request.release, request.out.as_deref());
    let command = build_command(request, &checkout_dir);
    let _out_lock = hold(&out)?;
    let root_dir = match &request.revision {
        None => checkout_dir,
        Some(revision) => {
            let (exported, commit) = revision::export(&checkout_dir, revision, &out)?;
            eprintln!("building commit {commit}, exported into {}", exported.display());
            // Stado's source contract: build scripts that stamp a revision
            // read it here, since the export carries no Git history.
            if std::env::var_os("WISENT_SOURCE_COMMIT").is_none() {
                std::env::set_var("WISENT_SOURCE_COMMIT", &commit);
            }
            exported
        }
    };
    let root = manifest::Manifest::read(&root_dir)?;
    let bins = binaries::wanted(&root, &request.bins)?;
    let lock_path = root_dir.join("Cargo.lock");
    let lock = lock::Lock::read(&lock_path)?;
    let profile = profile::Profile::read(&root_dir, request.release)?;
    let cache = match (&request.cache, std::env::var_os("KOMPILATOR_CACHE").filter(|cache| !cache.is_empty())) {
        (Some(cache), _) => cache.clone(),
        (None, Some(cache)) => PathBuf::from(cache),
        (None, None) => out.clone(),
    };
    let _cache_lock = if cache != out { Some(hold(&cache)?) } else { None };
    let rustc = compiler(&root_dir)?;
    let version = std::process::Command::new(&rustc)
        .arg("-vV")
        .output()
        .map_err(|error| format!("{rustc} could not run: {error}; name the compiler with RUSTC"))?;
    let rustc_version = String::from_utf8_lossy(&version.stdout).into_owned();
    let target = cfg::Target::host(&rustc)?;
    let graph = resolve::Graph::build(&lock, &root.name, &root_dir, &cache, &target, &request.features)?;
    let resolved = started.elapsed();
    let jobs = request.jobs.unwrap_or_else(|| std::thread::available_parallelism().map(|cores| cores.get()).unwrap_or(1));
    let settings = compile::Settings {
        rustc,
        rustc_version,
        profile,
        incremental: request.incremental,
        jobs,
        out: out.clone(),
        cache,
        bins: request.bins.clone(),
    };
    eprintln!(
        "{} {} ({}): {} packages, {} units, resolved in {:.2}s",
        root.name,
        root.version,
        settings.profile.name,
        graph.nodes.len(),
        graph.units.len(),
        resolved.as_secs_f64()
    );
    let (compiled, reused) = compile::build(&graph, &target, &settings)?;
    eprintln!(
        "built {} in {:.1}s: {compiled} units compiled, {reused} reused; binaries in {}",
        root.name,
        started.elapsed().as_secs_f64(),
        out.display()
    );
    // Every binary the build was asked for must now be there; its path is
    // the answer a caller reads, never one it spells itself.
    for path in binaries::located(&bins, &out, &command)? {
        println!("{}", path.display());
    }
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let request = match request(&arguments) {
        Ok(request) => request,
        Err(why) => {
            eprintln!("kompilator: {why}");
            return ExitCode::from(2);
        }
    };
    let done = match request.verb {
        Verb::Build => build(&request),
        Verb::Where => locate(&request),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("kompilator: {why}");
            ExitCode::FAILURE
        }
    }
}

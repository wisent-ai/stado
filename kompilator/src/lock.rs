//! The resolved graph a build compiles: `Cargo.lock`, each package's name,
//! version, source and the packages it depends on, and where each package's
//! source lies on this machine. A registry package is read from the
//! registry's unpacked sources, else downloaded once from crates.io and
//! checked against the checksum its lock entry pins; a Git package is
//! exported at its locked commit from Cargo's Git database, else from the
//! build's own database, fetched once from its URL; a package with no
//! source is read from its path.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::toml;

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// `registry+…`, `git+…#commit`, or none for a path package.
    pub source: Option<String>,
    /// Each dependency as the lock writes it: `name`, `name version` or
    /// `name version (source)`.
    pub dependencies: Vec<String>,
    /// The SHA-256 the lock pins for a registry package's archive.
    pub checksum: Option<String>,
}

impl Package {
    pub fn id(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

pub struct Lock {
    pub packages: Vec<Package>,
}

impl Lock {
    pub fn read(path: &Path) -> Result<Lock, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
        let document = toml::parse(&text).map_err(|why| format!("{} is not readable TOML: {why}", path.display()))?;
        let mut packages = Vec::new();
        for entry in document.get("package").and_then(toml::Value::as_array).into_iter().flatten() {
            let field = |key: &str| entry.get(key).and_then(toml::Value::as_str).map(String::from);
            let (Some(name), Some(version)) = (field("name"), field("version")) else {
                return Err(format!("{} holds a package without a name and version", path.display()));
            };
            packages.push(Package {
                name,
                version,
                source: field("source"),
                dependencies: entry.get("dependencies").map(toml::Value::strings).unwrap_or_default(),
                checksum: field("checksum"),
            });
        }
        Ok(Lock { packages })
    }

    /// The packages a dependency line of `from` names.
    pub fn resolve(&self, line: &str) -> Result<&Package, String> {
        let mut words = line.split_whitespace();
        let name = words.next().ok_or_else(|| format!("an empty dependency line"))?;
        let version = words.next();
        let candidates: Vec<&Package> = self
            .packages
            .iter()
            .filter(|package| package.name == name && version.is_none_or(|wanted| package.version == wanted))
            .collect();
        match candidates.as_slice() {
            [only] => Ok(only),
            [] => Err(format!("the lock names {line}, and no locked package is it")),
            _ => Err(format!("the lock names {line}, and several locked packages are it")),
        }
    }

    /// The root package: the one with no source whose directory holds the
    /// manifest the build was asked for.
    pub fn root(&self, name: &str) -> Result<&Package, String> {
        self.packages
            .iter()
            .find(|package| package.name == name && package.source.is_none())
            .ok_or_else(|| format!("the lock holds no path package named {name}"))
    }
}

/// Cargo's home: CARGO_HOME, else `~/.cargo`.
pub fn cargo_home() -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("CARGO_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set, so Cargo's home cannot be named")?;
    Ok(PathBuf::from(home).join(".cargo"))
}

/// Where the source of `package` lies: unpacked from the registry, exported
/// from the Git database at its commit into `cache`, or `root_dir` for the
/// root's path packages.
pub fn source_dir(package: &Package, root_dir: &Path, cache: &Path) -> Result<PathBuf, String> {
    let home = cargo_home()?;
    match package.source.as_deref() {
        None => Ok(root_dir.to_path_buf()),
        Some(source) if source.starts_with("registry+") || source.starts_with("sparse+") => {
            let base = home.join("registry/src");
            let wanted = format!("{}-{}", package.name, package.version);
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let candidate = entry.path().join(&wanted);
                    if candidate.join("Cargo.toml").exists() {
                        return Ok(candidate);
                    }
                }
            }
            registry_download(package, source, cache)
        }
        Some(source) if source.starts_with("git+") => git_source(package, source, &home, cache),
        Some(other) => Err(format!("{} comes from {other}, a source this build does not read", package.id())),
    }
}

/// The directory of `package` inside its Git repository exported at its
/// locked commit. The repository is Cargo's Git database for that URL.
fn git_source(package: &Package, source: &str, home: &Path, cache: &Path) -> Result<PathBuf, String> {
    let commit = source.rsplit('#').next().filter(|commit| commit.len() >= 7).ok_or_else(|| {
        format!("the Git source of {} names no commit: {source}", package.id())
    })?;
    let url = source.trim_start_matches("git+");
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let repository = url.trim_end_matches('/').trim_end_matches(".git").rsplit('/').next().unwrap_or(url);
    let export = cache.join("git").join(format!("{repository}-{commit}"));
    if !export.join(".exported").exists() {
        let holds = |path: &Path| {
            Command::new("git")
                .arg("--git-dir")
                .arg(path)
                .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let cargo_database = std::fs::read_dir(home.join("git/db")).ok().and_then(|entries| {
            entries.flatten().map(|entry| entry.path()).find(|path| {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                name.rsplit_once('-').is_some_and(|(stem, _)| stem == repository) && holds(path)
            })
        });
        let database = match cargo_database {
            Some(database) => database,
            None => {
                let own = cache.join("git/db").join(repository);
                if !holds(&own) {
                    git_fetch(&own, url, commit, package)?;
                }
                own
            }
        };
        let _ = std::fs::remove_dir_all(&export);
        std::fs::create_dir_all(&export).map_err(|error| format!("{} cannot be made: {error}", export.display()))?;
        let archive = Command::new("git")
            .arg("--git-dir")
            .arg(&database)
            .args(["archive", "--format=tar", commit])
            .output()
            .map_err(|error| format!("git archive of {commit} could not run: {error}"))?;
        if !archive.status.success() {
            return Err(format!(
                "git archive of {commit} in {} exited {}: {}",
                database.display(),
                archive.status,
                String::from_utf8_lossy(&archive.stderr).trim()
            ));
        }
        let tar = export.join(".archive.tar");
        std::fs::write(&tar, &archive.stdout).map_err(|error| format!("{} cannot be written: {error}", tar.display()))?;
        let unpacked = Command::new("tar")
            .arg("-xf")
            .arg(&tar)
            .arg("-C")
            .arg(&export)
            .status()
            .map_err(|error| format!("tar could not unpack {}: {error}", tar.display()))?;
        if !unpacked.success() {
            return Err(format!("tar could not unpack {}: exit {unpacked}", tar.display()));
        }
        let _ = std::fs::remove_file(&tar);
        std::fs::write(export.join(".exported"), commit)
            .map_err(|error| format!("{} cannot be marked exported: {error}", export.display()))?;
    }
    find_manifest(&export, &package.name)?.ok_or_else(|| {
        format!("no manifest of a package named {} stands in {} at {commit}", package.name, export.display())
    })
}

/// Fetch `commit` of `url` into the bare repository `database`, made when
/// missing. The commit is asked for by name first; a server that refuses
/// that is asked for every branch and tag.
fn git_fetch(database: &Path, url: &str, commit: &str, package: &Package) -> Result<(), String> {
    if !database.join("HEAD").exists() {
        std::fs::create_dir_all(database).map_err(|error| format!("{} cannot be made: {error}", database.display()))?;
        let made = Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .arg(database)
            .output()
            .map_err(|error| format!("git could not run to make {}: {error}; install Git", database.display()))?;
        if !made.status.success() {
            return Err(format!(
                "git init --bare {} exited {}: {}",
                database.display(),
                made.status,
                String::from_utf8_lossy(&made.stderr).trim()
            ));
        }
    }
    eprintln!("fetching {url} at {commit} for {}", package.id());
    let fetch = |refspec: &str| {
        Command::new("git")
            .arg("--git-dir")
            .arg(database)
            .args(["fetch", "--quiet", "--no-tags", url, refspec])
            .output()
            .map_err(|error| format!("git fetch could not run: {error}; install Git"))
    };
    let by_name = fetch(commit)?;
    if by_name.status.success() {
        return Ok(());
    }
    let everything = fetch("+refs/*:refs/remotes/origin/*")?;
    let present = Command::new("git")
        .arg("--git-dir")
        .arg(database)
        .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if everything.status.success() && present {
        return Ok(());
    }
    Err(format!(
        "{url} could not be fetched at {commit} for {}: asking for the commit said {}; asking for every branch and tag {}. \
         Check that the URL is reachable, that this machine's Git credentials may read it, and that the commit was pushed",
        package.id(),
        String::from_utf8_lossy(&by_name.stderr).trim(),
        if everything.status.success() {
            String::from("succeeded, and none of them holds it")
        } else {
            format!("said {}", String::from_utf8_lossy(&everything.stderr).trim())
        }
    ))
}

/// Download a crates.io package's archive once, check it against the
/// checksum its lock entry pins, and unpack it under `cache`.
fn registry_download(package: &Package, source: &str, cache: &Path) -> Result<PathBuf, String> {
    let crates_io =
        matches!(source, "registry+https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/");
    if !crates_io {
        return Err(format!(
            "{} comes from the registry {source}, and only crates.io is downloaded by this build; unpack it once with the registry's own client",
            package.id()
        ));
    }
    let checksum = package.checksum.as_deref().ok_or_else(|| {
        format!("the lock pins no checksum for {}, so its download could not be checked; lock it again", package.id())
    })?;
    let base = cache.join("registry");
    let unpacked = base.join(format!("{}-{}", package.name, package.version));
    if unpacked.join(".unpacked").exists() {
        return Ok(unpacked);
    }
    std::fs::create_dir_all(&base).map_err(|error| format!("{} cannot be made: {error}", base.display()))?;
    let url = format!("https://static.crates.io/crates/{0}/{0}-{1}.crate", package.name, package.version);
    eprintln!("downloading {url}");
    let download = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--retry", "3", &url])
        .output()
        .map_err(|error| format!("curl could not run to download {}: {error}; install curl", package.id()))?;
    if !download.status.success() {
        return Err(format!(
            "{url} could not be downloaded for {} ({}): {}",
            package.id(),
            download.status,
            String::from_utf8_lossy(&download.stderr).trim()
        ));
    }
    let digest = crate::sha256::hex(&download.stdout);
    if digest != checksum {
        return Err(format!(
            "{url} has SHA-256 {digest}, and the lock pins {checksum} for {}; the download is not the locked package",
            package.id()
        ));
    }
    let _ = std::fs::remove_dir_all(&unpacked);
    let archive = base.join(format!(".{}-{}.crate", package.name, package.version));
    std::fs::write(&archive, &download.stdout).map_err(|error| format!("{} cannot be written: {error}", archive.display()))?;
    let untarred = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&base)
        .output()
        .map_err(|error| format!("tar could not run to unpack {}: {error}", archive.display()))?;
    let _ = std::fs::remove_file(&archive);
    if !untarred.status.success() || !unpacked.join("Cargo.toml").exists() {
        return Err(format!(
            "{} did not unpack to {} ({}): {}",
            package.id(),
            unpacked.display(),
            untarred.status,
            String::from_utf8_lossy(&untarred.stderr).trim()
        ));
    }
    std::fs::write(unpacked.join(".unpacked"), checksum)
        .map_err(|error| format!("{} cannot be marked unpacked: {error}", unpacked.display()))?;
    Ok(unpacked)
}

/// The directory under `root` whose manifest names the package `name`.
fn find_manifest(root: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let manifest = dir.join("Cargo.toml");
        if manifest.exists() {
            let text = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{} cannot be read: {error}", manifest.display()))?;
            if let Ok(document) = toml::parse(&text) {
                if document.get("package").and_then(|package| package.get("name")).and_then(toml::Value::as_str)
                    == Some(name)
                {
                    return Ok(Some(dir));
                }
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let skip = path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with('.') || name == "target"
            });
            if path.is_dir() && !skip {
                pending.push(path);
            }
        }
    }
    Ok(None)
}


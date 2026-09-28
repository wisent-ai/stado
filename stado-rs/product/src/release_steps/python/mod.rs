//! `stado product python build` and `stado product python deliver-pypi`.
//!
//! A Python package is built by Python's own build frontend, as a Rust one is
//! by Cargo; what the copied `scripts/stado_release.py` added around it —
//! the reproducible environment, the one-wheel-one-sdist check, the bundle,
//! the verified upload and its evidence — lives here once.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::json;

use super::{archive_entry, output_dir, required, ARCHIVE_EPOCH};

mod zipapp;

/// The bundle a build stages and a delivery looks for inside the release.
const BUNDLE: &str = "python-distributions.tar";
/// PyPI's upload endpoint.
const PYPI_UPLOAD: &str = "https://upload.pypi.org/legacy/";

pub fn run(operation: &str, arguments: &clap::ArgMatches) -> Result<i32> {
    match operation {
        "build" => build(),
        "deliver-pypi" => deliver_pypi(),
        "zipapp" => {
            let packages: Vec<String> = arguments
                .get_many::<String>("package")
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            let text = |name: &str| {
                arguments
                    .get_one::<String>(name)
                    .cloned()
                    .with_context(|| format!("zipapp requires --{name}"))
            };
            zipapp::build(&packages, &text("module")?, &text("name")?, &python())
        }
        other => bail!("unknown Python release operation {other}"),
    }
}

fn python() -> String {
    std::env::var("WISENT_PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

fn run_checked(command: &mut Command) -> Result<()> {
    let rendered = format!("{command:?}");
    let status = command
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("cannot run {rendered}"))?;
    if !status.success() {
        bail!("{rendered} failed with {status}");
    }
    Ok(())
}

/// The distributions in `directory`: exactly one wheel and one sdist.
fn distributions(directory: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut wheels = Vec::new();
    let mut sdists = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.ends_with(".whl") {
            wheels.push(path);
        } else if name.ends_with(".tar.gz") {
            sdists.push(path);
        }
    }
    match (wheels.as_slice(), sdists.as_slice()) {
        ([wheel], [sdist]) => Ok((wheel.clone(), sdist.clone())),
        _ => bail!(
            "a Python release carries exactly one wheel and one sdist; {} holds {} wheel(s) and {} sdist(s)",
            directory.display(),
            wheels.len(),
            sdists.len()
        ),
    }
}

fn build() -> Result<i32> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    let version = required("WISENT_VERSION")?;
    let output = output_dir()?;
    let raw = output.join(format!("python-build-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&raw)?;
    let built = (|| -> Result<(PathBuf, PathBuf)> {
        run_checked(
            Command::new(python())
                .args([
                    "-m",
                    "build",
                    "--no-isolation",
                    "--sdist",
                    "--wheel",
                    "--outdir",
                ])
                .arg(&raw)
                .current_dir(&source)
                .env("SOURCE_DATE_EPOCH", ARCHIVE_EPOCH.to_string())
                .env("PYTHONHASHSEED", "0"),
        )?;
        distributions(&raw)
    })();
    let result = built.and_then(|(wheel, sdist)| {
        for artifact in [&wheel, &sdist] {
            let name = artifact.file_name().unwrap().to_string_lossy();
            if !name.contains(&version) {
                bail!("{name} does not carry WISENT_VERSION {version}");
            }
        }
        let release = output.join("release");
        if release.exists() {
            fs::remove_dir_all(&release)?;
        }
        fs::create_dir_all(&release)?;
        let bundle = release.join(BUNDLE);
        let mut archive = tar::Builder::new(fs::File::create(&bundle)?);
        let mut ordered = [wheel, sdist];
        ordered.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
        for artifact in &ordered {
            let name = artifact.file_name().unwrap().to_string_lossy();
            archive_entry(
                &mut archive,
                &format!("distributions/{name}"),
                &fs::read(artifact)?,
                false,
            )?;
        }
        archive.finish()?;
        println!("staged {}", bundle.display());
        Ok(0)
    });
    let _ = fs::remove_dir_all(&raw);
    result
}

/// Extract `archive` into `destination`, refusing links and members that
/// would land outside it.
pub(super) fn safe_unpack(archive: &Path, destination: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    let gzip = archive
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".gz") || name.ends_with(".tgz"));
    let reader: Box<dyn std::io::Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut entries = tar::Archive::new(reader);
    for entry in entries.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            bail!(
                "{} holds a link: {}",
                archive.display(),
                entry.path()?.display()
            );
        }
        if !entry.unpack_in(destination)? {
            bail!(
                "{} holds a member outside its root: {}",
                archive.display(),
                entry.path()?.display()
            );
        }
    }
    Ok(())
}

pub(super) fn find(directory: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            found.extend(find(&path, name)?);
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            found.push(path);
        }
    }
    Ok(found)
}

/// PyPI refuses a distribution whose metadata carries a direct reference
/// (`Requires-Dist: name @ git+https://…`) with a bare HTTP 400 after the
/// upload has started, and `twine check` does not catch it (pypa/twine#726).
/// wisent-gradio's delivery checked this before uploading; every Python
/// delivery does now.
fn refuse_direct_references(sdist: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(sdist)?));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.path()?.to_string_lossy().ends_with("PKG-INFO") {
            continue;
        }
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text)?;
        if let Some(line) = text
            .lines()
            .find(|line| line.starts_with("Requires-Dist:") && line.contains("@ "))
        {
            bail!(
                "{} declares a direct dependency, which PyPI rejects: {:?}. Publish that \
                 dependency to an index and require it by name and version, or keep this \
                 product off PyPI; nothing was uploaded",
                sdist.display(),
                line.trim()
            );
        }
    }
    Ok(())
}

fn deliver_pypi() -> Result<i32> {
    let token = required("PYPI_TOKEN")?;
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    let actual = crate::common::sha256(&archive)?;
    if actual != release_sha256 {
        bail!(
            "the release archive {} is {actual}, not the published {release_sha256}; nothing was uploaded",
            archive.display()
        );
    }
    let output = output_dir()?;
    let work = output.join(format!("pypi-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<Vec<String>> {
        let bundle = if archive.file_name().and_then(|n| n.to_str()) == Some(BUNDLE) {
            archive.clone()
        } else {
            let release = work.join("release");
            fs::create_dir_all(&release)?;
            safe_unpack(&archive, &release)?;
            match find(&release, BUNDLE)?.as_slice() {
                [bundle] => bundle.clone(),
                found => bail!(
                    "the release holds {} {BUNDLE} (one is required)",
                    found.len()
                ),
            }
        };
        let unpacked = work.join("distributions");
        fs::create_dir_all(&unpacked)?;
        safe_unpack(&bundle, &unpacked)?;
        let (wheel, sdist) = distributions(&unpacked.join("distributions"))?;
        refuse_direct_references(&sdist)?;
        run_checked(
            Command::new(python())
                .args([
                    "-m",
                    "twine",
                    "upload",
                    "--non-interactive",
                    "--repository-url",
                    PYPI_UPLOAD,
                ])
                .arg(&wheel)
                .arg(&sdist)
                .env("TWINE_USERNAME", "__token__")
                .env("TWINE_PASSWORD", &token),
        )?;
        Ok([wheel, sdist]
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect())
    })();
    let _ = fs::remove_dir_all(&work);
    let files = result?;
    let evidence = json!({
        "provider": "pypi",
        "product": required("WISENT_PRODUCT")?,
        "version": required("WISENT_VERSION")?,
        "release_uri": required("WISENT_RELEASE_URI")?,
        "release_sha256": release_sha256,
        "files": files,
    });
    fs::write(output.join("pypi-evidence.json"), format!("{evidence}\n"))?;
    println!("uploaded {} to PyPI", files.join(", "));
    Ok(0)
}

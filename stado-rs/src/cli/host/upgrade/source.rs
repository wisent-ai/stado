//! The revision an upgrade installs: origin/main's tip, fetched without
//! asking anyone for a credential, or a commit origin/main carries; never
//! one older than the revision the host runs.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::change::git;
use crate::cli::CmdError;

use super::installed::Installed;

/// The crate manifest inside the Stado checkout: the one source of the
/// version a build of a revision reports.
pub(super) const MANIFEST: &str = "stado-rs/Cargo.toml";

/// The revision chosen, where it was read, and the version it declares.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Source {
    pub checkout: PathBuf,
    pub commit: String,
    pub origin_main: String,
    pub version: String,
}

/// Whether the host needs the revision at all.
pub(super) enum Admitted {
    /// The host runs exactly this revision, built from a clean tree.
    Current,
    /// The revision descends from the one the host runs.
    Newer,
}

/// The canonical checkout of this product: the workspace's checkout of the
/// repository the catalog names for it, whose `origin` is that repository.
async fn checkout() -> Result<PathBuf, CmdError> {
    let runtime = stado_product::common::Runtime::new(None)
        .map_err(|error| CmdError::declaration(format!("{error:#}")))?;
    let catalog = stado_product::catalog::embedded()
        .map_err(|error| CmdError::declaration(format!("the compiled catalog: {error:#}")))?;
    let product = env!("CARGO_PKG_NAME");
    let repository = stado_product::catalog::product(&catalog, product)
        .and_then(|record| stado_product::catalog::text(record, "owner_repository"))
        .map_err(|error| CmdError::declaration(format!("{error:#}")))?
        .to_string();
    let name = match repository.rsplit_once('/') {
        Some((_, name)) => name,
        None => repository.as_str(),
    };
    let checkout = runtime.workspace.join(name);
    if !checkout.join(".git").exists() {
        return Err(CmdError::missing(format!(
            "{} is not a checkout of {repository}, the repository the catalog names for \
             {product}; the upgrade builds from that checkout (WISENT_WORKSPACE names the \
             workspace)",
            checkout.display()
        )));
    }
    let origin = git::repository(&checkout).await?;
    if !origin.eq_ignore_ascii_case(&repository) {
        return Err(CmdError::declaration(format!(
            "{}'s origin is {origin}, not {repository}, the repository the catalog names for \
             {product}",
            checkout.display()
        )));
    }
    Ok(checkout)
}

/// `package.version` of a crate manifest's text.
fn package_version(manifest: &str) -> Option<String> {
    let parsed: toml::Value = toml::from_str(manifest).ok()?;
    Some(parsed.get("package")?.get("version")?.as_str()?.to_string())
}

/// Fetch origin's main and choose the revision.
pub(super) async fn resolve(requested: Option<&str>) -> Result<Source, CmdError> {
    let checkout = checkout().await?;
    let origin_main = git::fetch_main(&checkout).await.map_err(|error| {
        error.within(format!(
            "fetching origin's main into {}",
            checkout.display()
        ))
    })?;
    let commit = match requested {
        None => origin_main.clone(),
        Some(revision) => {
            let full = git::commit(&checkout, revision).await?;
            if !git::contains(&checkout, &full, &origin_main).await? {
                return Err(CmdError::refused(format!(
                    "--source-commit {revision} ({full}) is not on origin/main ({origin_main}); \
                     an upgrade installs only a revision origin/main carries"
                )));
            }
            full
        }
    };
    let version = version_at(&checkout, &commit).await?;
    Ok(Source {
        checkout,
        commit,
        origin_main,
        version,
    })
}

async fn version_at(checkout: &Path, commit: &str) -> Result<String, CmdError> {
    let manifest = git::git(checkout, &["show", &format!("{commit}:{MANIFEST}")]).await?;
    package_version(&manifest).ok_or_else(|| {
        CmdError::declaration(format!(
            "{commit}:{MANIFEST} declares no package.version, so the version it builds is not \
             known"
        ))
    })
}

/// Refuse a revision older than, or beside, the one `host` runs.
pub(super) async fn admit(
    source: &Source,
    installed: &Installed,
    host: &str,
) -> Result<Admitted, CmdError> {
    let parse = |version: &str, whose: &str| {
        semver::Version::parse(version).map_err(|error| {
            CmdError::declaration(format!(
                "{whose} version {version:?} is not SemVer: {error}"
            ))
        })
    };
    let ours = parse(&source.version, &format!("{}'s", source.commit))?;
    let theirs = parse(&installed.version, &format!("{host}'s installed"))?;
    if ours < theirs {
        return Err(CmdError::refused(format!(
            "{} declares Stado {ours}, older than the {theirs} {host} runs ({}); an upgrade \
             never installs an older release",
            source.commit, installed.version_line
        )));
    }
    let Some(revision) = installed.revision.as_deref() else {
        return Err(CmdError::refused(format!(
            "{host} runs {}, which names no source revision, so whether {} is newer is not \
             known; nothing was changed",
            installed.version_line, source.commit
        )));
    };
    let running = git::commit(&source.checkout, revision)
        .await
        .map_err(|error| {
            error.within(format!(
                "{host} runs {}, a revision {} does not hold, so whether {} is newer is not known",
                installed.version_line,
                source.checkout.display(),
                source.commit
            ))
        })?;
    if running == source.commit && !installed.dirty {
        return Ok(Admitted::Current);
    }
    if !git::contains(&source.checkout, &running, &source.commit).await? {
        return Err(CmdError::refused(format!(
            "{host} runs {running} ({}), which {} does not descend from: the install would \
             take {host} back or sideways; name a later --source-commit",
            installed.version_line, source.commit
        )));
    }
    Ok(Admitted::Newer)
}

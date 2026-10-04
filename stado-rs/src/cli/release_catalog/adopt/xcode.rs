//! What an iOS checkout's own Xcode project says about the app it builds.
//!
//! Adoption reads the one `*.xcodeproj` at the checkout root instead of asking
//! the operator to retype it: the project name (also the default scheme), the
//! application's bundle identifier, the development team and the marketing
//! version the release manifest will read the product version from. A value
//! the project does not state plainly is a refusal that names it.

use std::path::Path;

use super::{fill, Planned, BUILD, MANIFEST, QUALITY};
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// The manifest and scripts an iOS checkout is released with, filled from
/// its project.
pub(super) fn files(
    checkout: &Path,
    product: &str,
    scheme: Option<&str>,
) -> Result<Vec<Planned>, CmdError> {
    let project = read(checkout)?;
    let scheme = scheme
        .map(str::to_string)
        .unwrap_or_else(|| project.name.clone());
    let values = [
        ("PRODUCT", product),
        ("PROJECT", project.name.as_str()),
        ("SCHEME", scheme.as_str()),
        ("APP", scheme.as_str()),
        ("BUNDLE_ID", project.bundle_id.as_str()),
        ("TEAM", project.team.as_str()),
    ];
    eprintln!(
        "{product}: {}.xcodeproj, scheme {scheme}, bundle {}, team {}, version {}",
        project.name, project.bundle_id, project.team, project.version
    );
    Ok(vec![
        Planned {
            path: checkout.join(PRODUCT_MANIFEST),
            text: fill(MANIFEST, &values),
            executable: false,
        },
        Planned {
            path: checkout.join("release/build.sh"),
            text: fill(BUILD, &values),
            executable: true,
        },
        Planned {
            path: checkout.join("release/quality.sh"),
            text: fill(QUALITY, &values),
            executable: true,
        },
    ])
}

pub(super) struct Project {
    pub name: String,
    pub bundle_id: String,
    pub team: String,
    pub version: String,
}

/// Every value an `IDENTIFIER = value;` line assigns, unquoted, in file order.
fn assigned(pbxproj: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} = ");
    pbxproj
        .lines()
        .filter_map(|line| line.trim().strip_prefix(&prefix))
        .map(|value| value.trim_end_matches(';').trim_matches('"').to_string())
        .collect()
}

/// The application's bundle identifier: test bundles and build-setting
/// references are not it, and an extension's identifier extends the app's, so
/// the shortest one left is the app.
fn application_bundle(pbxproj: &str) -> Option<String> {
    assigned(pbxproj, "PRODUCT_BUNDLE_IDENTIFIER")
        .into_iter()
        .filter(|id| !id.contains("$(") && !id.ends_with("Tests"))
        .min_by_key(String::len)
}

fn plain_version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(super) fn read(checkout: &Path) -> Result<Project, CmdError> {
    let mut projects = Vec::new();
    for entry in std::fs::read_dir(checkout)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "xcodeproj")
        {
            projects.push(path);
        }
    }
    let project = match projects.as_slice() {
        [one] => one,
        [] => {
            return Err(CmdError::refused(format!(
                "{} has no .xcodeproj at its root; --kind ios-xcode reads the app from it",
                checkout.display()
            )))
        }
        many => {
            let names: Vec<String> = many.iter().map(|path| path.display().to_string()).collect();
            return Err(CmdError::refused(format!(
                "{} holds several Xcode projects ({}); adopt reads exactly one",
                checkout.display(),
                names.join(", ")
            )));
        }
    };
    let name = project
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pbxproj = std::fs::read_to_string(project.join("project.pbxproj"))?;
    let bundle_id = application_bundle(&pbxproj).ok_or_else(|| {
        CmdError::click(format!(
            "{name}.xcodeproj states no application PRODUCT_BUNDLE_IDENTIFIER"
        ))
    })?;
    let team = assigned(&pbxproj, "DEVELOPMENT_TEAM")
        .into_iter()
        .find(|team| !team.is_empty() && !team.contains("$("))
        .ok_or_else(|| CmdError::click(format!("{name}.xcodeproj states no DEVELOPMENT_TEAM")))?;
    let versions = assigned(&pbxproj, "MARKETING_VERSION");
    let version = versions.first().cloned().unwrap_or_default();
    if versions.is_empty() || !versions.iter().all(|value| plain_version(value)) {
        return Err(CmdError::refused(format!(
            "{name}.xcodeproj must state MARKETING_VERSION as two or three numbers \
             (1.0 or 1.0.0) for the release to read its version; it states {versions:?}"
        )));
    }
    Ok(Project {
        name,
        bundle_id,
        team,
        version,
    })
}

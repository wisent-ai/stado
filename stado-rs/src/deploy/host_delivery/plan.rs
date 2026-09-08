//! Local decisions taken before any host is contacted: the destination
//! policy, the shape of the local source, and the delivery plan they yield.

use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};

use uuid::Uuid;

use crate::deploy::DeployError;

const MANAGED_RUNS_ROOT: &str = ".stado/work/runs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceKind {
    File,
    Directory,
}

impl SourceKind {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug)]
pub(super) struct DeliveryPlan {
    pub(super) source: String,
    pub(super) destination: String,
    pub(super) kind: SourceKind,
    pub(super) root_mode: u32,
    pub(super) file_list: Option<String>,
}

fn safe_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Validate the destination without consulting a host. Delivery is confined
/// to one run UUID below Stado's managed work root; the final component names
/// the source tree or application bundle within that run.
fn destination_components(destination: &str) -> Result<Vec<&str>, DeployError> {
    if destination.starts_with('/') || destination.starts_with('~') || destination.contains('\0') {
        return Err(DeployError(format!(
            "destination {destination:?} is outside the managed area; use a path relative to the approved account's home under {MANAGED_RUNS_ROOT}/<RUN-UUID>/"
        )));
    }
    let path = Path::new(destination);
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .ok_or_else(|| DeployError("destination must be UTF-8".to_string())),
            _ => Err(DeployError(format!(
                "destination {destination:?} must contain only ordinary path components and no '..'"
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty()
        || components
            .iter()
            .any(|component| !safe_component(component))
    {
        return Err(DeployError(format!(
            "destination {destination:?} must contain only path components made of letters, digits, '.', '_' or '-' and no '..'"
        )));
    }
    let runs: Vec<&str> = MANAGED_RUNS_ROOT.split('/').collect();
    if !components.starts_with(&runs) || components.len() < runs.len() + 2 {
        return Err(DeployError(format!(
            "destination {destination:?} is outside the managed area; use a path relative to the approved account's home under {MANAGED_RUNS_ROOT}/<RUN-UUID>/"
        )));
    }
    let run = components[runs.len()];
    let canonical = Uuid::parse_str(run)
        .ok()
        .map(|value| value.hyphenated().to_string())
        .is_some_and(|value| value == run);
    if !canonical {
        return Err(DeployError(format!(
            "destination {destination:?} does not name a canonical lowercase UUID below {MANAGED_RUNS_ROOT}"
        )));
    }
    Ok(components)
}

fn validate_file_list(raw: Option<&str>, kind: SourceKind) -> Result<Option<String>, DeployError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if kind != SourceKind::Directory {
        return Err(DeployError(
            "--files-from is valid only when SOURCE is a directory".to_string(),
        ));
    }
    if raw.is_empty() {
        return Err(DeployError(
            "--files-from is empty; refusing an accidental empty-tree replacement".to_string(),
        ));
    }
    if !raw.ends_with('\0') {
        return Err(DeployError(
            "--files-from must be NUL-delimited and end with NUL".to_string(),
        ));
    }
    for entry in raw[..raw.len() - 1].split('\0') {
        let path = Path::new(entry);
        if entry.is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                !matches!(component, Component::Normal(_))
                    || matches!(component, Component::ParentDir)
            })
        {
            return Err(DeployError(format!(
                "--files-from entry {entry:?} is not a relative path below SOURCE"
            )));
        }
    }
    Ok(Some(raw.to_string()))
}

pub(super) fn plan(
    source: &str,
    destination: &str,
    file_list: Option<&str>,
) -> Result<DeliveryPlan, DeployError> {
    let components = destination_components(destination)?;
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| DeployError(format!("cannot read delivery source {source:?}: {error}")))?;
    if metadata.file_type().is_symlink() {
        return Err(DeployError(
            "delivery source must be a regular file or directory, not a symlink".to_string(),
        ));
    }
    let kind = if metadata.is_file() {
        SourceKind::File
    } else if metadata.is_dir() {
        SourceKind::Directory
    } else {
        return Err(DeployError(
            "delivery source must be a regular file or directory".to_string(),
        ));
    };
    let destination = components.join("/");
    Ok(DeliveryPlan {
        source: source.to_string(),
        destination,
        kind,
        root_mode: metadata.permissions().mode() & 0o7777,
        file_list: validate_file_list(file_list, kind)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probierz_destination_requires_a_canonical_run_uuid_and_child() {
        assert!(destination_components(
            ".stado/work/runs/123e4567-e89b-12d3-a456-426614174000/probierz"
        )
        .is_ok());
        assert!(destination_components(".stado/work/runs/not-a-uuid/probierz").is_err());
        assert!(
            destination_components(".stado/work/runs/123e4567-e89b-12d3-a456-426614174000")
                .is_err()
        );
    }

    #[test]
    fn destinations_outside_managed_run_root_are_refused() {
        assert!(destination_components("tmp/tree").is_err());
        assert!(destination_components("/tmp/tree").is_err());
        assert!(destination_components(".stado/work/tree").is_err());
    }
}

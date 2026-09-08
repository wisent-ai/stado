//! Every refusal a well-shaped declaration document can still earn.

use super::declaration::{Declaration, Install, Readback};
use super::{
    DECLARATION_PATH, PLATFORMS, SCHEMA_VERSION, TARGET_PLACEHOLDER, UNIT_LAUNCHD, UNIT_SYSTEMD,
};

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// One path component that is safe to bind into a remote program: letters,
/// digits, `.`, `_` and `-`, and never `.` or `..` alone.
fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A relative path of safe components, with no leading, trailing or empty
/// component and no `..` anywhere in it.
fn safe_relative_path(value: &str) -> bool {
    !value.is_empty() && value.split('/').all(safe_segment)
}

/// `$HOME/<relative>`: every install root is inside the account that runs
/// the product, so a delivery cannot be pointed at `/` or another user.
fn home_path(value: &str) -> bool {
    value.strip_prefix("$HOME/").is_some_and(safe_relative_path)
}

/// A unit-file path: `$HOME`-relative, or absolute for a system domain.
fn unit_path(value: &str) -> bool {
    home_path(value) || value.strip_prefix('/').is_some_and(safe_relative_path)
}

/// Every refusal the declaration itself can earn.
///
/// serde has already refused a document with a missing or unknown field, so
/// these are the rules a well-shaped document can still break. Each one names
/// the product and the field, because the reader is somebody adding a product
/// and the useful answer is which line to fix.
pub fn validate(declaration: &Declaration) -> Result<(), String> {
    if declaration.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}, and this build reads {SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.products.is_empty() {
        return Err(format!("{DECLARATION_PATH} declares no products"));
    }
    for (index, entry) in declaration.products.iter().enumerate() {
        let refuse = |detail: String| -> String {
            format!(
                "{DECLARATION_PATH} products[{index}] ({}): {detail}",
                entry.name
            )
        };
        if !safe_segment(&entry.name) {
            return Err(refuse(
                "name must be a bare token of letters, digits, '.', '_' and '-'".to_string(),
            ));
        }
        if declaration
            .products
            .iter()
            .filter(|other| other.name == entry.name)
            .count()
            != 1
        {
            return Err(refuse("name is declared twice".to_string()));
        }
        if entry.why.trim().is_empty() {
            return Err(refuse(
                "why must say what this product is and what runs it; a refusal prints it"
                    .to_string(),
            ));
        }
        if !safe_segment(&entry.source.product) {
            return Err(refuse(
                "source.product becomes a release coordinate segment and must be a bare token"
                    .to_string(),
            ));
        }
        if !safe_relative_path(&entry.source.member) {
            return Err(refuse(
                "source.member must be a relative archive path with no '..' component".to_string(),
            ));
        }
        if entry.platforms.is_empty() {
            return Err(refuse("platforms must name at least one".to_string()));
        }
        for platform in &entry.platforms {
            if !PLATFORMS.contains(&platform.as_str()) {
                return Err(refuse(format!(
                    "platform {platform:?} is not one of {}",
                    PLATFORMS.join(", ")
                )));
            }
            if entry
                .platforms
                .iter()
                .filter(|other| *other == platform)
                .count()
                != 1
            {
                return Err(refuse(format!("platform {platform:?} is declared twice")));
            }
        }
        if !home_path(entry.root()) {
            return Err(refuse(
                "install.root must be a $HOME-relative path inside the account that runs it"
                    .to_string(),
            ));
        }
        for preserved in entry.install.preserve() {
            if !safe_relative_path(preserved) {
                return Err(refuse(format!(
                    "preserved path {preserved:?} must be relative to the install root, with no \
                     '..' component"
                )));
            }
            if entry
                .install
                .preserve()
                .iter()
                .filter(|other| *other == preserved)
                .count()
                != 1
            {
                return Err(refuse(format!(
                    "preserved path {preserved:?} is declared twice"
                )));
            }
        }
        match (&entry.install, &entry.readback) {
            (Install::Program { .. }, Readback::Program { argument, shape: _ }) => {
                if argument.trim().is_empty()
                    || argument.bytes().any(|byte| byte.is_ascii_whitespace())
                {
                    return Err(refuse(
                        "version.argument must be one whitespace-free argument".to_string(),
                    ));
                }
            }
            (Install::Tree { .. }, Readback::JsonFile { path, pointer }) => {
                if !safe_relative_path(path) {
                    return Err(refuse(
                        "version.path must be a file relative to the install root".to_string(),
                    ));
                }
                let Some(member) = pointer.strip_prefix('/') else {
                    return Err(refuse(
                        "version.pointer must address one top-level member, as '/version'"
                            .to_string(),
                    ));
                };
                if !safe_segment(member) {
                    return Err(refuse(
                        "version.pointer must address one top-level member, as '/version'"
                            .to_string(),
                    ));
                }
                // The version source is code, so replacing the code must
                // replace it. A version read out of a preserved path would
                // report the old build forever after a successful delivery.
                if entry.install.preserve().iter().any(|preserved| {
                    path == preserved || path.starts_with(&format!("{preserved}/"))
                }) {
                    return Err(refuse(format!(
                        "version.path {path:?} is inside a preserved path, so a delivery could \
                         never change the version it reports"
                    )));
                }
            }
            (Install::Program { .. }, Readback::JsonFile { .. }) => {
                return Err(refuse(
                    "a program's version is read by running it, not out of a file beside it"
                        .to_string(),
                ));
            }
            (Install::Tree { .. }, Readback::Program { .. }) => {
                return Err(refuse(
                    "a tree's version must be read from a file inside it, because there is no one \
                     installed program to ask"
                        .to_string(),
                ));
            }
        }
        for (unit_index, unit) in entry.units.iter().enumerate() {
            let label = unit.label_for("target");
            if !safe_segment(&label) {
                return Err(refuse(format!(
                    "units[{unit_index}].label must be a bare unit name, optionally carrying \
                     '{{target}}'"
                )));
            }
            if entry
                .units
                .iter()
                .filter(|other| other.label_for("target") == label)
                .count()
                != 1
            {
                return Err(refuse(format!(
                    "units[{unit_index}].label {label:?} is declared more than once"
                )));
            }
            match (&unit.kind, &unit.path) {
                (None, None) => {}
                (Some(kind), Some(path)) => {
                    if kind != UNIT_LAUNCHD && kind != UNIT_SYSTEMD {
                        return Err(refuse(format!(
                            "units[{unit_index}].kind {kind:?} must be {UNIT_LAUNCHD} or \
                             {UNIT_SYSTEMD}"
                        )));
                    }
                    if !unit_path(&path.replace(TARGET_PLACEHOLDER, "target")) {
                        return Err(refuse(format!(
                            "units[{unit_index}].path must be a $HOME-relative or absolute \
                             unit-file path with no '..' component"
                        )));
                    }
                }
                _ => {
                    return Err(refuse(format!(
                        "units[{unit_index}].kind and units[{unit_index}].path locate the unit \
                         file together; declare both or neither, and a label alone is confirmed \
                         against the registry"
                    )));
                }
            }
        }
    }
    Ok(())
}

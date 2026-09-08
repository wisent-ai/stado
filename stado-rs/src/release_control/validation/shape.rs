//! The shapes a release document's names, paths and digests must have.

use std::path::{Component, Path};

use crate::release::canonical_coordinate;

/// A canonical coordinate: ASCII alphanumerics plus `.`, `_` and `-`, no
/// surrounding whitespace, non-empty.
///
/// Crate-visible because the unit-image revisit policy validates launchd
/// labels against exactly this shape, and a second spelling of "what a
/// canonical name may contain" is a second answer waiting to disagree with
/// this one.
pub(crate) fn identifier(value: &str) -> bool {
    canonical_coordinate(value)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(super) fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

pub(super) fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// An absolute path with no control characters and no `..` component.
///
/// Crate-visible for the same reason [`identifier`] is: the unit-image revisit
/// policy declares its own `state_dir` and holds it to exactly this shape,
/// and a second spelling of "a safe absolute path" is a second answer.
pub(crate) fn safe_absolute(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        && !value.chars().any(char::is_control)
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

pub(super) fn safe_install_root(value: &str) -> bool {
    if value == "{home}" {
        return true;
    }
    if let Some(relative) = value.strip_prefix("{home}/") {
        return safe_relative(relative);
    }
    !value.contains("{home}") && (safe_absolute(value) || safe_relative(value))
}

pub(super) fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

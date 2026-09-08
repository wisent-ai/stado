//! The one-line predicates every validator in this module agrees on, and the
//! serde defaults the schema fills omitted fields with.

use std::path::{Component, Path};

pub(in crate::release_pipeline) fn default_required() -> bool {
    true
}

pub(in crate::release_pipeline) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(in crate::release_pipeline) fn default_extract() -> bool {
    true
}

pub(in crate::release_pipeline) fn platform_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
}

pub(in crate::release_pipeline) fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

pub fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.chars().any(char::is_control)
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(in crate::release_pipeline) fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(in crate::release_pipeline) fn argv(value: &[String]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|part| !part.is_empty() && !part.as_bytes().contains(&0))
}

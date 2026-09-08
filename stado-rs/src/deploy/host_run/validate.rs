//! Lexical refusals applied to caller text before registry or network
//! access. The host repeats the path check against its own real `$HOME`.

use crate::deploy::py_str_repr;

use super::{PATH_REFUSAL, RUN_AREA};

/// Reject an argument that cannot possibly be inside the target's managed run
/// area before registry or network access. The host still binds the candidate
/// to its own `$HOME`; this lexical check does not guess that home locally.
pub fn validate_run_descendant(path: &str) -> Result<(), String> {
    let components = std::path::Path::new(path).components().collect::<Vec<_>>();
    let ordinary = components
        .iter()
        .skip(1)
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    let managed = path
        .split_once(&format!("/{RUN_AREA}/"))
        .is_some_and(|(home, relative)| !home.is_empty() && !relative.is_empty());
    if path.starts_with('/') && !path.contains('\0') && ordinary && managed {
        Ok(())
    } else {
        Err(format!("path {} {PATH_REFUSAL}", py_str_repr(path)))
    }
}

/// A recursive delete addresses one complete run, never the shared run root or
/// one nested subtree. Build and execution accept deeper descendants.
pub fn validate_run_directory(path: &str) -> Result<(), String> {
    validate_run_descendant(path)?;
    let relative = path
        .split_once(&format!("/{RUN_AREA}/"))
        .map(|(_, relative)| relative)
        .unwrap_or_default();
    let safe_name = !relative.is_empty()
        && !relative.contains('/')
        && relative
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe_name {
        Ok(())
    } else {
        Err(format!(
            "run directory {} must be one direct, safely named child of the target account's $HOME/{RUN_AREA}",
            py_str_repr(path)
        ))
    }
}

pub fn validate_binary_name(binary: &str) -> Result<(), String> {
    let safe = binary
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && binary
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe {
        Ok(())
    } else {
        Err(format!(
            "binary name {} must start with a letter or number and contain only letters, numbers, dot, dash, or underscore",
            py_str_repr(binary)
        ))
    }
}

pub fn validate_arguments(arguments: &[String]) -> Result<(), String> {
    if let Some(argument) = arguments.iter().find(|argument| argument.contains('\0')) {
        Err(format!(
            "program argument {} carries a NUL byte",
            py_str_repr(argument)
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_paths_are_bounded_to_one_managed_tree() {
        assert!(validate_run_descendant("/Users/dev/.stado/work/runs/abc/src/Cargo.toml").is_ok());
        assert!(validate_run_descendant("/tmp/Cargo.toml").is_err());
        assert!(validate_run_descendant("/Users/dev/.stado/work/runs/../secret").is_err());
        assert!(validate_run_directory("/Users/dev/.stado/work/runs/abc").is_ok());
        assert!(validate_run_directory("/Users/dev/.stado/work/runs/abc/src").is_err());
    }
}

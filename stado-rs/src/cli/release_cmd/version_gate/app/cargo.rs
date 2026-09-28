//! A Rust command-line product's surface and version, read statically from
//! its source so the gate runs on a tree that does not build and a published
//! tag is measured with the same ruler as the candidate.
//!
//! `--command-table FILE:TABLE` names the one static table the binary prints
//! as its advertised command list (`static TABLE: &[...] = &[ ... ];`); every
//! `name: "..."` field of it is a `command:` name. `--cargo-toml` names the
//! Cargo.toml whose `[package]` `version` is the declared version. A command
//! that dispatches without a row in the table is private: nothing may depend
//! on a spelling the gate cannot see.

use std::collections::BTreeSet;

use super::surface::{Loader, Read};

const NAME_FIELD: &str = "name:";
const TABLE_OPENING: &str = "= &[";
const TABLE_CLOSER: &str = "];";
const PACKAGE_SECTION: &str = "[package]";
const VERSION_KEY: &str = "version";

fn utf8(load: Loader, path: &str) -> Read<String> {
    String::from_utf8(load(path)?).map_err(|error| format!("{path}: not UTF-8 ({error})"))
}

/// The contents of one plain `"..."` literal, refusing escapes and
/// interpolation a static read cannot resolve.
fn literal(path: &str, line: &str, value: &str) -> Read<String> {
    value
        .trim()
        .trim_end_matches(',')
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|inner| !inner.is_empty() && !inner.contains(['"', '\\', '{']))
        .map(str::to_string)
        .ok_or_else(|| format!("{path}: `{line}` is not one plain string literal"))
}

/// `command:<name>` for every row of the table `spec` (`FILE:TABLE`) names.
pub(super) fn commands(load: Loader, spec: &str) -> Read<Vec<String>> {
    let (path, table) = spec
        .split_once(':')
        .filter(|(path, table)| !path.is_empty() && !table.is_empty())
        .ok_or_else(|| format!("--command-table {spec:?} is not FILE:TABLE"))?;
    let source = utf8(load, path)?;
    let opener = format!("static {table}: ");
    let mut lines = source.lines().map(str::trim);
    lines
        .by_ref()
        .find(|line| line.starts_with(&opener) && line.ends_with(TABLE_OPENING))
        .ok_or_else(|| format!("{path}: no `static {table}: &[...] {TABLE_OPENING}` table"))?;
    let mut names = Vec::new();
    for line in lines {
        if line == TABLE_CLOSER {
            let unique: BTreeSet<&String> = names.iter().collect();
            if names.is_empty() {
                return Err(format!("{path}: table {table} names no command"));
            }
            if unique.len() != names.len() {
                return Err(format!("{path}: table {table} names a command twice"));
            }
            return Ok(names.iter().map(|name| format!("command:{name}")).collect());
        }
        if let Some(value) = line.strip_prefix(NAME_FIELD) {
            names.push(literal(path, line, value)?);
        }
    }
    Err(format!(
        "{path}: table {table} is never closed by `{TABLE_CLOSER}`"
    ))
}

/// The `version` of the `[package]` section, as written.
pub(super) fn declared_version(load: Loader, path: &str) -> Read<String> {
    let manifest = utf8(load, path)?;
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == PACKAGE_SECTION;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if in_package && key.trim() == VERSION_KEY {
            return literal(path, line, value);
        }
    }
    Err(format!(
        "{path}: {PACKAGE_SECTION} declares no literal {VERSION_KEY}"
    ))
}

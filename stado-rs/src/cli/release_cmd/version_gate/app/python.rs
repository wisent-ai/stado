//! A Python distribution's surface and version, read statically from its
//! sources so the gate never imports the package: a release decision must
//! not depend on a machine that can import its dependencies, and the same
//! reader measures a published tag's blobs and the candidate tree.
//!
//! `--pyproject` names the pyproject.toml whose `[project]` `version` is the
//! declared version and whose `[project.scripts]` keys are `console-script:`
//! names. `--setup-py` names a setup.py whose `setup(version="...")` is the
//! declared version. `--python-all` names a module whose `__all__` entries
//! are `api:` names. `--python-argparse` names a module whose
//! `add_parser("name", ..., help=...)` calls are `cli:` names: a subparser
//! without `help=` dispatches but is unlisted, and unlisted is private.
//! `--python-manifests DIR` with `--manifest-suffix` walks DIR for modules
//! assigning a dict or list literal to a module-level constant whose name
//! ends with one of the suffixes; each string key is `<family>:<key>`, the
//! family being the first directory under DIR the module sits in.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::surface::{Loader, Read};

const PROJECT_SECTION: &str = "[project]";
const SCRIPTS_SECTION: &str = "[project.scripts]";
const VERSION_KEY: &str = "version";
const ALL_OPENER: &str = "__all__";
const OTHER_FAMILY: &str = "other";

static SETUP_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bversion\s*=\s*["'](?P<version>[^"']+)["']"#).expect("valid"));
static ADD_PARSER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)add_parser\(\s*["'](?P<name>[^"']+)["'](?P<rest>[^)]*)\)"#).expect("valid")
});
static HELP_KEYWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bhelp\s*=").expect("valid"));
static STRING_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^["'](?P<value>[^"']*)["']"#).expect("valid"));
static CONSTANT_OPENER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<name>[A-Z][A-Z0-9_]*)\s*(?::[^=]+)?=\s*(?P<bracket>[\[{])").expect("valid")
});

fn utf8(load: Loader, path: &str) -> Read<String> {
    String::from_utf8(load(path)?).map_err(|error| format!("{path}: not UTF-8 ({error})"))
}

/// The contents of one plain TOML string, refusing anything else.
fn toml_string(path: &str, line: &str, value: &str) -> Read<String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        });
    inner
        .filter(|inner| !inner.is_empty() && !inner.contains(['"', '\\']))
        .map(str::to_string)
        .ok_or_else(|| format!("{path}: `{line}` is not one plain string"))
}

/// `(version, console scripts)` of a pyproject.toml's `[project]` table.
fn pyproject(load: Loader, path: &str) -> Read<(Option<String>, Vec<String>)> {
    let text = utf8(load, path)?;
    let mut version = None;
    let mut scripts = Vec::new();
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        if section == PROJECT_SECTION && key == VERSION_KEY {
            version = Some(toml_string(path, line, value)?);
        }
        if section == SCRIPTS_SECTION && !key.is_empty() {
            toml_string(path, line, value)?;
            scripts.push(format!("console-script:{key}"));
        }
    }
    Ok((version, scripts))
}

/// The `[project]` version a pyproject.toml declares.
pub(super) fn pyproject_version(load: Loader, path: &str) -> Read<String> {
    pyproject(load, path)?
        .0
        .ok_or_else(|| format!("{path}: {PROJECT_SECTION} declares no literal {VERSION_KEY}"))
}

/// The `console-script:` names a pyproject.toml declares.
pub(super) fn console_scripts(load: Loader, path: &str) -> Read<Vec<String>> {
    Ok(pyproject(load, path)?.1)
}

/// The `version="..."` a setup.py passes to `setup(...)`.
pub(super) fn setup_version(load: Loader, path: &str) -> Read<String> {
    let text = utf8(load, path)?;
    let mut found = SETUP_VERSION
        .captures_iter(&text)
        .map(|capture| capture["version"].to_string());
    let version = found
        .next()
        .ok_or_else(|| format!("{path}: no literal `version=\"...\"` in setup()"))?;
    if found.next().is_some() {
        return Err(format!("{path}: more than one literal `version=` ; which one setup() receives cannot be read statically"));
    }
    Ok(version)
}

/// The strings of `__all__`, as `api:` names.
pub(super) fn exported(load: Loader, path: &str) -> Read<Vec<String>> {
    let text = utf8(load, path)?;
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if !inside {
            let Some(rest) = line.strip_prefix(ALL_OPENER) else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            inside = true;
            depth = 0;
            push_strings(rest.trim(), &mut depth, &mut names);
        } else {
            push_strings(line, &mut depth, &mut names);
        }
        if inside && depth == 0 {
            break;
        }
    }
    if !inside {
        return Err(format!("{path}: declares no `{ALL_OPENER} = [...]`"));
    }
    if names.is_empty() {
        return Err(format!("{path}: `{ALL_OPENER}` names nothing"));
    }
    Ok(names
        .into_iter()
        .map(|name| format!("api:{name}"))
        .collect())
}

/// Every leading string literal of the comma-separated pieces of `line`,
/// tracking bracket depth so the caller knows when the literal closes.
fn push_strings(line: &str, depth: &mut usize, names: &mut Vec<String>) {
    let code = line.split('#').next().unwrap_or("");
    for piece in code.split(',') {
        let piece = piece.trim().trim_start_matches(['[', '(', '{']);
        if let Some(capture) = STRING_LITERAL.captures(piece) {
            names.push(capture["value"].to_string());
        }
    }
    *depth += code.matches(['[', '(', '{']).count();
    *depth = depth.saturating_sub(code.matches([']', ')', '}']).count());
}

/// The `cli:` names of the subparsers a module registers with a `help=`.
pub(super) fn cli_commands(load: Loader, path: &str) -> Read<Vec<String>> {
    let text = utf8(load, path)?;
    let names: Vec<String> = ADD_PARSER
        .captures_iter(&text)
        .filter(|capture| HELP_KEYWORD.is_match(&capture["rest"]))
        .map(|capture| format!("cli:{}", &capture["name"]))
        .collect();
    if names.is_empty() {
        return Err(format!("{path}: registers no subparser with a help text"));
    }
    Ok(names)
}

/// The string keys of the dict or list literals assigned to constants whose
/// name ends with one of `suffixes`, in one module.
fn manifest_keys(text: &str, suffixes: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut inside = false;
    for line in text.lines() {
        if !inside {
            let Some(capture) = CONSTANT_OPENER.captures(line) else {
                continue;
            };
            if !suffixes
                .iter()
                .any(|suffix| capture["name"].ends_with(suffix.as_str()))
            {
                continue;
            }
            inside = true;
            depth = 0;
            let after = &line[capture.get(0).map_or(0, |whole| whole.end() - 1)..];
            push_strings(after, &mut depth, &mut keys);
        } else {
            push_strings(line.trim(), &mut depth, &mut keys);
        }
        if inside && depth == 0 {
            inside = false;
        }
    }
    keys
}

/// `<family>:<key>` for every manifest key under `dir`.
pub(super) fn manifests(load: Loader, dir: &str, suffixes: &[String]) -> Read<Vec<String>> {
    if suffixes.is_empty() {
        return Err(format!(
            "--python-manifests {dir} needs at least one --manifest-suffix"
        ));
    }
    let dir = dir.trim_end_matches('/');
    let mut names = BTreeSet::new();
    let mut pending = vec![dir.to_string()];
    while let Some(folder) = pending.pop() {
        for entry in super::surface::entries(load, &folder)? {
            let path = format!("{folder}/{}", entry.trim_end_matches('/'));
            if entry.ends_with('/') {
                pending.push(path);
                continue;
            }
            if !entry.ends_with(".py") {
                continue;
            }
            let family = path[dir.len() + 1..]
                .split_once('/')
                .map_or(OTHER_FAMILY, |(first, _)| first);
            for key in manifest_keys(&utf8(load, &path)?, suffixes) {
                names.insert(format!("{family}:{key}"));
            }
        }
    }
    if names.is_empty() {
        return Err(format!("{dir}: no constant ending with {suffixes:?} assigns a literal with string keys; the manifests moved or stopped being literals, which changes what the package promises"));
    }
    Ok(names.into_iter().collect())
}

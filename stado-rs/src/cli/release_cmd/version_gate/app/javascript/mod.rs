//! A JavaScript package's surface beyond its package.json: the named exports
//! of every module its `exports` map opens (`api:`), the commands its CLI's
//! usage text lists (`cmd:`, `--usage-commands FILE`) or its command array
//! names (`cmd:`, `--command-array FILE:ARRAY`), and the tools its MCP server
//! lists (`mcp:`, `--mcp-tools FILE`).
//!
//! Each file is read through one masking pass that blanks comments and the
//! bodies of string, template and regular-expression literals, so a keyword
//! inside a string never reads as code and a brace inside one never unbalances
//! a span. A file the pass cannot read with certainty (an unclosed `${` in a
//! template, unbalanced braces or brackets after masking) is refused, never
//! read as a shorter surface.

use std::sync::LazyLock;

use regex::Regex;

mod mask;
pub(super) mod package;

use super::surface::{Loader, Read};
use mask::masked;

const USAGE_MARKER: &str = "USAGE";
const COMMANDS_HEADING: &str = "commands:";
const TOOLS_MARKER: &str = "TOOLS";
const NAME_KEY: &str = "name:";
const DEFAULT_EXPORT: &str = "default";

/// An exported declaration: `export`, the words that declare it, then the
/// name, which is followed by what opens its value, parameters or body.
static EXPORT_DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bexport\s+((?:[A-Za-z]+\s*\*?\s+)*?)([A-Za-z_$][\w$]*)\s*(?:[=({;,]|\bextends\b)")
        .expect("the export declaration pattern compiles")
});
static EXPORT_LIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bexport\s*\{([^}]*)\}").expect("the export list pattern compiles")
});
static USAGE_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ {2}([a-z][a-z0-9-]*)(?:\s|$)").expect("the usage command pattern compiles")
});

/// `api:<name>` for every named export of the module at `path`.
pub(super) fn exports(load: Loader, path: &str) -> Read<Vec<String>> {
    let code = masked(load, path)?.code;
    let mut names = Vec::new();
    for found in EXPORT_DECLARATION.captures_iter(&code) {
        if found[1].split_whitespace().next() == Some(DEFAULT_EXPORT) {
            return Err(format!(
                "{path}: a default export has no name a caller writes; name it"
            ));
        }
        names.push(found[2].to_string());
    }
    for list in EXPORT_LIST.captures_iter(&code) {
        names.extend(
            list[1]
                .split(',')
                .filter_map(|item| item.split_whitespace().last())
                .map(str::to_string),
        );
    }
    if names.iter().any(|name| name == DEFAULT_EXPORT) {
        return Err(format!(
            "{path}: a default export has no name a caller writes; name it"
        ));
    }
    Ok(names
        .into_iter()
        .map(|name| format!("api:{name}"))
        .collect())
}

/// `cmd:<name>` for every command the literal after `USAGE` lists under its
/// `commands:` heading, up to the first blank line after the first command.
pub(super) fn usage_commands(load: Loader, path: &str) -> Read<Vec<String>> {
    let file = masked(load, path)?;
    let start = file
        .code
        .find(USAGE_MARKER)
        .ok_or_else(|| format!("{path}: no {USAGE_MARKER}"))?;
    let usage = file
        .literals
        .iter()
        .find(|(offset, _)| *offset > start)
        .map(|(_, body)| body.as_str())
        .ok_or_else(|| format!("{path}: no literal after {USAGE_MARKER}"))?;
    let (_, listing) = usage.split_once(COMMANDS_HEADING).ok_or_else(|| {
        format!("{path}: the literal after {USAGE_MARKER} has no `{COMMANDS_HEADING}` section")
    })?;
    let mut names = Vec::new();
    for line in listing.lines() {
        if line.trim().is_empty() {
            if names.is_empty() {
                continue;
            }
            break;
        }
        if let Some(found) = USAGE_COMMAND.captures(line) {
            names.push(format!("cmd:{}", &found[1]));
        }
    }
    if names.is_empty() {
        return Err(format!("{path}: `{COMMANDS_HEADING}` lists no command"));
    }
    Ok(names)
}

/// `api:` names of every module the package.json at `package` maps a
/// subpath to with a plain path. A subpath whose target is a condition map
/// counts as an `export:` name through `--export-conditions`, but its modules
/// are not read here: which file a caller gets depends on its conditions.
pub(super) fn package_exports(load: Loader, package: &str) -> Read<Vec<String>> {
    let document: serde_json::Value = serde_json::from_slice(&load(package)?)
        .map_err(|error| format!("{package}: not JSON ({error})"))?;
    let targets: Vec<&str> = match &document["exports"] {
        serde_json::Value::String(path) => vec![path.as_str()],
        serde_json::Value::Object(map) => {
            map.values().filter_map(|target| target.as_str()).collect()
        }
        _ => Vec::new(),
    };
    if targets.is_empty() {
        return Err(format!(
            "{package}: exports maps no subpath to a module path"
        ));
    }
    let base = std::path::Path::new(package)
        .parent()
        .unwrap_or(std::path::Path::new(""));
    let mut names = Vec::new();
    for target in targets {
        let module = base.join(target.trim_start_matches("./"));
        names.extend(exports(load, &module.to_string_lossy())?);
    }
    Ok(names)
}

/// The names `--js-exports`, `--usage-commands`, `--command-array` and
/// `--mcp-tools` add.
pub(super) fn surface(load: Loader, sources: &super::AppSources) -> Read<Vec<String>> {
    let mut names = Vec::new();
    if let (true, Some(package)) = (sources.js_exports, &sources.package_json) {
        names.extend(package_exports(load, package)?);
    }
    if let Some(cli) = &sources.usage_commands {
        names.extend(usage_commands(load, cli)?);
    }
    if let Some(table) = &sources.command_array {
        names.extend(command_array(load, table)?);
    }
    if let Some(server) = &sources.mcp_tools {
        names.extend(mcp_tools(load, server)?);
    }
    Ok(names)
}

/// `mcp:<name>` for every `name:` string inside the array after `TOOLS`.
pub(super) fn mcp_tools(load: Loader, path: &str) -> Read<Vec<String>> {
    named_strings(load, path, TOOLS_MARKER, "tool").map(|names| {
        names
            .into_iter()
            .map(|name| format!("mcp:{name}"))
            .collect()
    })
}

/// `cmd:<name>` for every `name:` string inside the array `FILE:ARRAY`
/// names.
pub(super) fn command_array(load: Loader, table: &str) -> Read<Vec<String>> {
    let (path, array) = table
        .rsplit_once(':')
        .filter(|(path, array)| !path.is_empty() && !array.is_empty())
        .ok_or_else(|| format!("--command-array takes FILE:ARRAY, not {table:?}"))?;
    named_strings(load, path, array, "command").map(|names| {
        names
            .into_iter()
            .map(|name| format!("cmd:{name}"))
            .collect()
    })
}

/// Every `name:` string inside the array after the first `marker` in the
/// code of `path`; `what` names one entry in the refusal for an empty array.
fn named_strings(load: Loader, path: &str, marker: &str, what: &str) -> Read<Vec<String>> {
    let file = masked(load, path)?;
    let at = file
        .code
        .find(marker)
        .ok_or_else(|| format!("{path}: no {marker}"))?;
    let open = file.code[at..]
        .find('[')
        .map(|offset| at + offset)
        .ok_or_else(|| format!("{path}: no `[` after {marker}"))?;
    let mut depth = 0usize;
    let mut close = None;
    for (offset, character) in file.code[open..].char_indices() {
        match character {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| format!("{path}: the {marker} array is never closed"))?;
    let names: Vec<String> = file
        .literals
        .iter()
        .filter(|(offset, _)| {
            (open..close).contains(offset) && file.code[..*offset].trim_end().ends_with(NAME_KEY)
        })
        .map(|(_, body)| body.to_string())
        .collect();
    if names.is_empty() {
        return Err(format!("{path}: the {marker} array names no {what}"));
    }
    Ok(names)
}

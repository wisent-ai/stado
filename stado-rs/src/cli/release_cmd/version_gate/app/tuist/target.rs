//! One entry of the manifest's `targets:` array, resolved far enough to be
//! trusted: its name, product kind and bundle identifier, and for a shipping
//! target the Info.plist keys and entitlements it declares, whichever spelling
//! the manifest uses for them.

use std::sync::LazyLock;

use plist::{Dictionary, Value};
use regex::Regex;

use super::arguments::{call, labelled, member, required, spelled};
use super::text::literal::{string_argument, Literal};
use super::text::{anchored, balanced, char_index, chars, find, Read};
use super::Project;

const LABEL_WITH: &str = "with";
const LABEL_PATH: &str = "path";

static TARGET_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.target\s*\(").expect("valid"));

/// Whether a Tuist product kind is installed on the device as an addressable
/// bundle; `None` for a kind this reader does not know. An unknown kind is an
/// error rather than a silent exclusion: guessing "it does not ship" is how a
/// surface shrinks by accident.
pub(crate) fn ships(product: &str) -> Option<bool> {
    match product {
        "app"
        | "appClip"
        | "appExtension"
        | "extensionKitExtension"
        | "messagesExtension"
        | "stickerPackExtension"
        | "tvTopShelfExtension"
        | "watch2App"
        | "watch2Extension" => Some(true),
        "bundle" | "commandLineTool" | "dynamicLibrary" | "framework" | "macro"
        | "staticFramework" | "staticLibrary" | "uiTests" | "unitTests" => Some(false),
        _ => None,
    }
}

fn plist_bytes(blob: &[u8], origin: &str) -> Read<Dictionary> {
    let parsed = Value::from_reader(std::io::Cursor::new(blob))
        .map_err(|error| format!("{origin}: does not parse as a plist: {error}"))?;
    parsed
        .into_dictionary()
        .ok_or_else(|| format!("{origin}: the top level is not a dictionary"))
}

fn file(arguments: &[char], project: &Project, origin: &str, what: &str) -> Read<Dictionary> {
    let labels = labelled(arguments, origin)?;
    let path = required(&labels, LABEL_PATH, origin, what)?;
    let relative = project.beside(&string_argument(path, origin, what)?);
    plist_bytes(&(project.load)(&relative)?, &relative)
}

fn dictionary(text: &[char], origin: &str, spelling: &str) -> Read<Dictionary> {
    Literal::new(text, origin)
        .parse()?
        .into_dictionary()
        .ok_or_else(|| format!("{origin}: {spelling} was not given a dictionary literal"))
}

/// The Info.plist keys a target declares.
pub(crate) fn info_plist(text: &[char], project: &Project, origin: &str) -> Read<Dictionary> {
    if member(text).as_deref() == Some("default") {
        return Ok(Dictionary::new());
    }
    for spelling in ["extendingDefault", "dictionary"] {
        let Some(arguments) = call(text, spelling, origin)? else {
            continue;
        };
        let labels = labelled(&arguments, origin)?;
        let body = labels.get(LABEL_WITH).unwrap_or(&arguments);
        return dictionary(body, origin, &format!(".{spelling}"));
    }
    if let Some(arguments) = call(text, "file", origin)? {
        return file(&arguments, project, origin, "infoPlist .file");
    }
    Err(format!(
        "{origin}: infoPlist is spelled {:?}, which this reader cannot read statically. Its keys are unknown, not absent.",
        spelled(text)
    ))
}

/// The entitlements a target declares.
pub(crate) fn entitlements(text: &[char], project: &Project, origin: &str) -> Read<Dictionary> {
    if let Some(arguments) = call(text, "file", origin)? {
        return file(&arguments, project, origin, "entitlements .file");
    }
    if let Some(arguments) = call(text, "dictionary", origin)? {
        return dictionary(&arguments, origin, "entitlements .dictionary");
    }
    Err(format!(
        "{origin}: entitlements is spelled {:?}, which this reader cannot read statically, so the capabilities it grants are unknown.",
        spelled(text)
    ))
}

/// The `.target(...)` text an entry stands for, and the file it was read from.
/// A bare `.member` entry is a `static let member: Target = .target(...)`
/// declared in one of the helper files the app names (`--tuist-helper`).
pub(crate) fn resolved(
    body: &[char],
    project: &Project,
    origin: &str,
) -> Read<(Vec<char>, String)> {
    if anchored(&TARGET_CALL, body).is_some() {
        return Ok((body.to_vec(), origin.to_string()));
    }
    let Some(name) = member(body) else {
        return Err(format!(
            "{origin}: a `targets:` entry is spelled {:?} rather than .target(...) or a .member helper, so what it ships is unknown",
            spelled(body)
        ));
    };
    let declaration = Regex::new(&format!(
        r"\bstatic\s+let\s+{}\s*:\s*Target\s*=\s*\.target\s*\(",
        regex::escape(&name)
    ))
    .expect("an escaped identifier forms a valid pattern");
    for helper in project.helpers {
        let source = String::from_utf8((project.load)(helper)?)
            .map_err(|error| format!("{helper}: not UTF-8 ({error})"))?;
        let Some(found) = declaration.find(&source) else {
            continue;
        };
        let text = chars(&source);
        let from = char_index(&source, found.start());
        let start = find(&text, from, ".target").expect("the declaration pattern contains .target");
        let opening = find(&text, start, "(")
            .expect("the declaration pattern contains an opening parenthesis");
        let end = balanced(&text, opening, helper)?;
        return Ok((text[start..end].to_vec(), helper.clone()));
    }
    Err(format!(
        "{origin}: the `targets:` entry .{name} is declared in none of the helper files named with --tuist-helper ({}), so what it ships is unknown",
        if project.helpers.is_empty() {
            "none named".to_string()
        } else {
            project.helpers.join(", ")
        }
    ))
}

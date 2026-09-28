//! The public surface of an app built from a Tuist `Project.swift`
//! (`--tuist-project`), read statically out of the manifest, the helper files
//! its `.member` targets are declared in (`--tuist-helper`), and the Info.plist
//! and entitlements files it names, relative to the manifest's directory.
//!
//! Nothing here runs `tuist generate`, builds, or needs a Mac: the same reader
//! runs against a tree recovered from a published tag, and the loader is the
//! only difference between reading a checkout and reading a tag.
//!
//! The names are what a user and the system hold of each shipping bundle:
//! `app-bundle-id:`/`extension-bundle-id:` (a changed one orphans installed
//! copies), `url-scheme:<bundle>:` (OAuth redirects and deep links already
//! sent), `app-group:` and `keychain-group:` (where an installed copy's data
//! sits), `associated-domain:` (universal links), `entitlement:` per key and
//! bundle, `extension-point:`, `shortcut-item:` and `localization:`. Purpose
//! strings, fonts, ad identifiers, signing and the marketing version itself
//! are left out: a user would not notice them disappear.
//!
//! Anything that will not parse is an error, never a shorter list: a target
//! must yield a name and a product kind this reader knows, an entitlements
//! path must resolve to a file that parses as a plist, and a shipping target
//! must stamp `$(MARKETING_VERSION)` into `CFBundleShortVersionString`, so the
//! version reported here is the version the artifact carries.

mod arguments;
mod names;
mod target;
mod text;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use plist::{Dictionary, Value};
use regex::Regex;

use text::literal::string_argument;
use text::{balanced, char_index, chars, skip_trivia, split_top_level, trimmed, Read};

const APP_PRODUCT: &str = "app";
const KEY_SHORT_VERSION: &str = "CFBundleShortVersionString";
const MARKETING_VERSION_VARIABLE: &str = "$(MARKETING_VERSION)";
const STEP: usize = 1;
/// Where a piece of manifest text starts.
const TEXT_START: usize = 0;
/// A manifest declares exactly this many of `targets:`, `.app` targets and
/// marketing versions.
const EXACTLY_ONE: usize = 1;

/// The one shape a marketing version is declared in.
static MARKETING_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.marketingVersion\("(?P<version>[^"]+)"\)"#).expect("valid"));
static TARGETS_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\btargets\s*:\s*\[").expect("valid"));

/// Maps a repository-relative path to its bytes: the working tree's, or a
/// published tag's through `git show`.
pub(crate) type Loader<'a> = &'a dyn Fn(&str) -> Read<Vec<u8>>;

/// Where the manifest's files are read from.
pub(crate) struct Project<'a> {
    load: Loader<'a>,
    /// The manifest's directory, repository-relative; empty at the root.
    dir: String,
    helpers: &'a [String],
}

impl Project<'_> {
    /// A path the manifest names, as Tuist resolves it: beside the manifest.
    fn beside(&self, relative: &str) -> String {
        if self.dir.is_empty() {
            relative.to_string()
        } else {
            format!("{}/{relative}", self.dir)
        }
    }
}

pub(super) struct Target {
    name: String,
    product: String,
    bundle_id: String,
    info: Dictionary,
    entitlements: Dictionary,
    /// The one `.marketingVersion("...")` a shipping target's own text sets.
    version: String,
}

fn targets_array(manifest: &str, origin: &str) -> Read<Vec<char>> {
    let found = TARGETS_LABEL.find_iter(manifest).collect::<Vec<_>>();
    let [label] = found.as_slice() else {
        return Err(format!(
            "{origin}: found {} `targets:` arrays, expected {EXACTLY_ONE}. The manifest's shape changed, so which targets ship is unknown rather than fewer.",
            found.len()
        ));
    };
    let text = chars(manifest);
    let opening = char_index(manifest, label.end()) - STEP;
    let end = balanced(&text, opening, origin)?;
    Ok(text[opening + STEP..end - STEP].to_vec())
}

fn one_target(piece: &[char], project: &Project, origin: &str) -> Read<Target> {
    let start = skip_trivia(piece, TEXT_START, origin)?;
    let (body, from) = target::resolved(trimmed(&piece[start..]), project, origin)?;
    let opening = body
        .iter()
        .position(|&character| character == '(')
        .expect("a resolved target is a call");
    let end = balanced(&body, opening, &from)?;
    let labels = arguments::labelled(&body[opening + STEP..end - STEP], &from)?;
    let name = string_argument(
        arguments::required(&labels, "name", &from, "a target")?,
        &from,
        "a target name",
    )?;
    let at = format!("{from}: target {name}");
    let product_text = arguments::required(&labels, "product", &from, "a target")?;
    let product = arguments::member(product_text).ok_or_else(|| {
        format!(
            "{at} has product {:?}, which is not a plain kind",
            arguments::spelled(product_text)
        )
    })?;
    let Some(shipping) = target::ships(&product) else {
        return Err(format!(
            "{at} has product '{product}', a kind this reader does not know. Whether it reaches a device is unknown, and guessing that it does not would shrink the surface."
        ));
    };
    let bundle_id = string_argument(
        arguments::required(&labels, "bundleId", &from, "a target")?,
        &at,
        "a bundleId",
    )?;
    let mut declared = Target {
        name,
        product,
        bundle_id,
        info: Dictionary::new(),
        entitlements: Dictionary::new(),
        version: String::new(),
    };
    if !shipping {
        return Ok(declared);
    }
    if let Some(text) = labels.get("infoPlist") {
        declared.info = target::info_plist(text, project, &at)?;
    }
    match declared.info.get(KEY_SHORT_VERSION) {
        Some(Value::String(stamped)) if stamped == MARKETING_VERSION_VARIABLE => {}
        other => {
            return Err(format!(
                "{at} stamps {KEY_SHORT_VERSION}={other:?} instead of {MARKETING_VERSION_VARIABLE:?}, so the version this artifact carries is not the version the manifest declares and nothing here can be checked against it."
            ))
        }
    }
    if let Some(text) = labels.get("entitlements") {
        declared.entitlements = target::entitlements(text, project, &at)?;
    }
    // The version is the one this target's own settings stamp, wherever the
    // target is declared (the manifest or a helper file).
    let own = text::string(&body[opening..end]);
    let versions = MARKETING_VERSION
        .captures_iter(&own)
        .map(|found| found["version"].to_string())
        .collect::<BTreeSet<_>>();
    let [version] = versions.iter().collect::<Vec<_>>()[..] else {
        return Err(format!(
            "{at} stamps {MARKETING_VERSION_VARIABLE} but its own settings set {} distinct .marketingVersion(...) values ({}), expected {EXACTLY_ONE}, so the version it carries is unknown.",
            versions.len(),
            versions.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    };
    declared.version = version.clone();
    Ok(declared)
}

/// Every target the manifest declares, resolved far enough to be trusted.
fn parse_targets(manifest: &str, project: &Project, origin: &str) -> Read<Vec<Target>> {
    let array = targets_array(manifest, origin)?;
    let found = split_top_level(&array, origin)?
        .iter()
        .map(|piece| one_target(piece, project, origin))
        .collect::<Read<Vec<_>>>()?;
    if found.is_empty() {
        return Err(format!("{origin}: the `targets:` array is empty"));
    }
    let apps = found
        .iter()
        .filter(|declared| declared.product == APP_PRODUCT)
        .count();
    if apps != EXACTLY_ONE {
        return Err(format!(
            "{origin}: found {apps} targets with product .app, expected {EXACTLY_ONE}. The App Store record belongs to exactly one application, so which artifact carries the version would otherwise be undefined."
        ));
    }
    Ok(found)
}

fn manifest_text(load: Loader, manifest: &str) -> Read<String> {
    String::from_utf8(load(manifest)?).map_err(|error| format!("{manifest}: not UTF-8 ({error})"))
}

/// Every shipping target of one tree, wherever its bytes come from:
/// `manifest` is the repository-relative `Project.swift`, `helpers` the files
/// its `.member` targets are declared in.
fn shipping(load: Loader, manifest: &str, helpers: &[String]) -> Read<Vec<Target>> {
    let text = manifest_text(load, manifest)?;
    let project = Project {
        load,
        dir: Path::new(manifest)
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default(),
        helpers,
    };
    Ok(parse_targets(&text, &project, manifest)?
        .into_iter()
        .filter(|declared| target::ships(&declared.product) == Some(true))
        .collect())
}

/// The surface of one tree.
pub(crate) fn surface(load: Loader, manifest: &str, helpers: &[String]) -> Read<Vec<String>> {
    let mut names = BTreeSet::new();
    for declared in shipping(load, manifest, helpers)? {
        names.extend(names::target_names(&declared, manifest)?);
    }
    if names.is_empty() {
        return Err(format!(
            "{manifest}: no shipping target yielded a name, so the surface is unknown"
        ));
    }
    Ok(names.into_iter().collect())
}

/// The one marketing version every shipping target sets. The App Store
/// refuses an extension whose version differs from its app's, so targets
/// that disagree are refused by name rather than one of them being chosen.
pub(crate) fn declared_version(load: Loader, manifest: &str, helpers: &[String]) -> Read<String> {
    let targets = shipping(load, manifest, helpers)?;
    let versions = targets
        .iter()
        .map(|declared| declared.version.as_str())
        .collect::<BTreeSet<_>>();
    match versions.iter().collect::<Vec<_>>()[..] {
        [version] => Ok(version.to_string()),
        _ => Err(format!(
            "{manifest}: its shipping targets set different marketing versions ({}), so the version the artifact carries is ambiguous; every shipping target must set the same one",
            targets
                .iter()
                .map(|declared| format!("{}={}", declared.name, declared.version))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

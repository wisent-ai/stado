//! Everything a user, a third party or iOS itself can address one shipping
//! bundle by: its bundle identifier, URL schemes, localizations, Home Screen
//! quick actions, extension point, and entitlement keys, with the values of
//! the three entitlements whose values are addresses (app groups, keychain
//! groups, associated domains). Each name is namespaced by bundle identifier,
//! because losing the App Group only in the widget is a real regression a flat
//! set of keys would hide behind the app's own copy.

use plist::{Dictionary, Value};

use super::text::Read;
use super::Target;

const KEY_URL_TYPES: &str = "CFBundleURLTypes";
const KEY_URL_SCHEMES: &str = "CFBundleURLSchemes";
const KEY_LOCALIZATIONS: &str = "CFBundleLocalizations";
const KEY_SHORTCUT_ITEMS: &str = "UIApplicationShortcutItems";
const KEY_SHORTCUT_TYPE: &str = "UIApplicationShortcutItemType";
/// Where each kind of extension declares the point the system loads it at.
const EXTENSION_POINTS: [(&str, &str); 2] = [
    ("NSExtension", "NSExtensionPointIdentifier"),
    ("EXAppExtensionAttributes", "EXExtensionPointIdentifier"),
];

/// The name kind of an entitlement whose values are addresses rather than
/// settings; every other entitlement contributes its key alone.
fn address_kind(entitlement: &str) -> Option<&'static str> {
    match entitlement {
        "com.apple.security.application-groups" => Some("app-group"),
        "keychain-access-groups" => Some("keychain-group"),
        "com.apple.developer.associated-domains" => Some("associated-domain"),
        _ => None,
    }
}

fn strings_under(value: Option<&Value>, key: &str, origin: &str) -> Read<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("{origin}: {key} is not an array"))?;
    entries
        .iter()
        .map(|entry| match entry.as_string().map(str::trim) {
            Some(text) if !text.is_empty() => Ok(text.to_string()),
            _ => Err(format!(
                "{origin}: an entry of {key} is not a non-empty string"
            )),
        })
        .collect()
}

fn dicts_under<'a>(value: Option<&'a Value>, key: &str, origin: &str) -> Read<Vec<&'a Dictionary>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("{origin}: {key} is not an array"))?;
    entries
        .iter()
        .map(|entry| {
            entry
                .as_dictionary()
                .ok_or_else(|| format!("{origin}: an entry of {key} is not a dictionary"))
        })
        .collect()
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub(super) fn target_names(target: &Target, origin: &str) -> Read<Vec<String>> {
    let bundle = &target.bundle_id;
    let at = format!("{origin}: target {}", target.name);
    let kind = if target.product == super::APP_PRODUCT {
        "app-bundle-id"
    } else {
        "extension-bundle-id"
    };
    let mut names = vec![format!("{kind}:{bundle}")];
    let info = &target.info;

    // An absent CFBundleURLTypes is a legitimate — and breaking — removal of
    // every scheme, so it yields no names; a malformed one is an error.
    for entry in dicts_under(info.get(KEY_URL_TYPES), KEY_URL_TYPES, &at)? {
        for scheme in strings_under(entry.get(KEY_URL_SCHEMES), KEY_URL_SCHEMES, &at)? {
            names.push(format!("url-scheme:{bundle}:{scheme}"));
        }
    }
    for code in strings_under(info.get(KEY_LOCALIZATIONS), KEY_LOCALIZATIONS, &at)? {
        names.push(format!("localization:{bundle}:{code}"));
    }
    for entry in dicts_under(info.get(KEY_SHORTCUT_ITEMS), KEY_SHORTCUT_ITEMS, &at)? {
        let kind = non_empty(entry.get(KEY_SHORTCUT_TYPE)).ok_or_else(|| {
            format!("{at}: a {KEY_SHORTCUT_ITEMS} entry declares no {KEY_SHORTCUT_TYPE}")
        })?;
        names.push(format!("shortcut-item:{bundle}:{kind}"));
    }
    for (holder, point) in EXTENSION_POINTS {
        let Some(attributes) = info.get(holder) else {
            continue;
        };
        let attributes = attributes
            .as_dictionary()
            .ok_or_else(|| format!("{at}: {holder} is not a dictionary"))?;
        let identifier = non_empty(attributes.get(point)).ok_or_else(|| {
            format!("{at}: {holder} declares no {point}, so the system cannot load it")
        })?;
        names.push(format!("extension-point:{bundle}:{identifier}"));
    }
    for (key, value) in target.entitlements.iter() {
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("{at}: an entitlement key is empty"));
        }
        names.push(format!("entitlement:{bundle}:{key}"));
        if let Some(kind) = address_kind(key) {
            for entry in strings_under(Some(value), key, &at)? {
                names.push(format!("{kind}:{bundle}:{entry}"));
            }
        }
    }
    Ok(names)
}

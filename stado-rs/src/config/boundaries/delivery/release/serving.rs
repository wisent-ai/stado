//! The serving API's publisher lookups: which declared publisher covers one
//! key or listing prefix, read from the loaded table and, for a product
//! declared since this process loaded its configuration, from the file as it
//! is on disk now.

use std::collections::BTreeMap;

use super::publishers::{parse_release_publishers, release_api_publishers, ReleasePublisher};

/// The serving API's publisher for `key`. A product declared after this
/// process loaded its configuration is looked up again in the file as it is
/// on disk now, so `ensure_publisher`'s declaration takes effect on the
/// serving hosts without restarting them; the declared table is still the
/// only authority, and a key no declaration names is still refused.
/// `Err` carries the refusal of the loaded table or of the fresh file, so a
/// declaration that cannot be read is never reported as a key nobody
/// publishes.
pub fn release_publisher_for_key(key: &str) -> Result<Option<ReleasePublisher>, String> {
    let find = |publishers: &BTreeMap<String, ReleasePublisher>| {
        publishers
            .values()
            .find(|publisher| publisher.allows_key(key))
            .cloned()
    };
    match find(release_api_publishers().map_err(|problems| problems.join("; "))?) {
        Some(publisher) => Ok(Some(publisher)),
        None => Ok(fresh_release_publishers()?.as_ref().and_then(find)),
    }
}

/// The serving API's publisher for a listing under `prefix`, with the
/// listing prefix it authorizes; `Err` as [`release_publisher_for_key`].
pub fn release_publisher_for_list(
    prefix: &str,
) -> Result<Option<(ReleasePublisher, String)>, String> {
    let find = |publishers: &BTreeMap<String, ReleasePublisher>| {
        publishers.values().find_map(|publisher| {
            publisher
                .authorized_list_prefix(prefix)
                .map(|authorized| (publisher.clone(), authorized))
        })
    };
    match find(release_api_publishers().map_err(|problems| problems.join("; "))?) {
        Some(found) => Ok(Some(found)),
        None => Ok(fresh_release_publishers()?.as_ref().and_then(find)),
    }
}

/// The publisher table in the config file as it is now, when the table is
/// read from the file at all: `Ok(None)` while `WC_RELEASE_API_PUBLISHERS`
/// pins it for the process's lifetime, `Err` when the file cannot be read
/// or the table it holds is refused.
fn fresh_release_publishers() -> Result<Option<BTreeMap<String, ReleasePublisher>>, String> {
    let pinned = match std::env::var("WC_RELEASE_API_PUBLISHERS") {
        Ok(value) => !value.trim().is_empty(),
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(raw)) => {
            return Err(format!("WC_RELEASE_API_PUBLISHERS is not Unicode: {raw:?}"))
        }
    };
    if pinned {
        return Ok(None);
    }
    let fresh = crate::config_file::load_config_file_fresh()
        .map_err(|error| format!("the configuration file cannot be re-read: {error}"))?;
    let declared = fresh
        .get("release_api")
        .and_then(|section| section.get("publishers"));
    parse_release_publishers(declared)
        .map(Some)
        .map_err(|problems| problems.join("; "))
}

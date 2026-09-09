use super::*;

pub(crate) fn validate_disk_cleanup(
    value: &Value,
    location: &str,
) -> Result<(), RegistryValidationError> {
    let map = value
        .as_object()
        .ok_or_else(|| verr(location, "must be an object"))?;
    const REQUIRED: [&str; 8] = [
        "check_interval_seconds",
        "cleaners",
        "low_free_gb",
        "max_bytes_per_pass",
        "max_items_per_pass",
        "max_scan_items",
        "mode",
        "target_free_gb",
    ];
    // Optional, and deliberately so: a registry that predates this key must
    // stay valid, and a host that says nothing keeps the janitor's own
    // 30-second pass deadline.
    const OPTIONAL: [&str; 1] = ["max_pass_seconds"];
    let keys: HashSet<&str> = map.keys().map(String::as_str).collect();
    let required: HashSet<&str> = REQUIRED.into_iter().collect();
    let allowed: HashSet<&str> = REQUIRED.into_iter().chain(OPTIONAL).collect();
    if !required.is_subset(&keys) || !keys.is_subset(&allowed) {
        return Err(verr(
            location,
            &format!(
                "must contain exactly {}, and may add {}",
                py_list_repr(&REQUIRED),
                py_list_repr(&OPTIONAL)
            ),
        ));
    }
    if let Some(declared) = map.get("max_pass_seconds") {
        // Upper bound so one pass cannot outlive its own interval: the
        // shortest `check_interval_seconds` this validator accepts is 60, and
        // a pass that ran longer than its interval would overlap itself and
        // meet its own lock.
        require_int(
            declared,
            &format!("{location}.max_pass_seconds"),
            1,
            Some(600),
        )?;
    }
    let mode_location = format!("{location}.mode");
    if !matches!(map["mode"].as_str(), Some("off" | "report" | "enforce")) {
        return Err(verr(
            &mode_location,
            "must be one of 'off', 'report', or 'enforce'",
        ));
    }
    require_int(
        &map["check_interval_seconds"],
        &format!("{location}.check_interval_seconds"),
        60,
        Some(86400),
    )?;
    let low = require_int(
        &map["low_free_gb"],
        &format!("{location}.low_free_gb"),
        1,
        None,
    )?;
    let target = require_int(
        &map["target_free_gb"],
        &format!("{location}.target_free_gb"),
        1,
        None,
    )?;
    if target <= low {
        return Err(verr(
            &format!("{location}.target_free_gb"),
            "must be greater than low_free_gb",
        ));
    }
    require_int(
        &map["max_bytes_per_pass"],
        &format!("{location}.max_bytes_per_pass"),
        1024_i64.pow(2),
        Some(1024_i64.pow(4)),
    )?;
    let max_items = require_int(
        &map["max_items_per_pass"],
        &format!("{location}.max_items_per_pass"),
        1,
        Some(10000),
    )?;
    let max_scan = require_int(
        &map["max_scan_items"],
        &format!("{location}.max_scan_items"),
        1,
        Some(MAX_SCAN_ITEMS_CEILING),
    )?;
    if max_scan < max_items {
        return Err(verr(
            &format!("{location}.max_scan_items"),
            "must be >= max_items_per_pass",
        ));
    }
    let cleaners_location = format!("{location}.cleaners");
    let cleaners = map["cleaners"]
        .as_object()
        .ok_or_else(|| verr(&cleaners_location, "must be an object"))?;
    // A cleaner this binary does not know is a cleaner a newer binary does:
    // the registry is one document read by every release in the fleet at
    // once. Refusing the whole policy for one unfamiliar name switched off
    // every cleaner on charless-mac-mini on 2026-09-04 the moment
    // `release_store` was declared for the binary that was still queued to
    // reach it — the janitor read `cleaners: null`, reported
    // `invalid_or_unavailable_policy`, and the disk it had been holding above
    // the watermark was left to fill. So an unknown name is skipped here and
    // reported by the janitor as `unknown_cleaner`; the known ones keep
    // running, and the new one starts the moment the binary that knows it
    // lands. A name is still held to the cleaner key schema below.
    let known: Vec<&str> = cleaners
        .keys()
        .map(String::as_str)
        .filter(|name| crate::providers::local::disk_cleanup::catalogue::cleaner(name).is_some())
        .collect();
    // An armed policy with no cleaner is a declaration that cannot act. It
    // passes every other check here: the mode is legal, the thresholds are
    // legal, the cleaner map is a legal empty object — and the janitor then
    // reports a healthy no-op on any disk, because there is nothing enabled
    // to find anything. `lukasz-macbook` carried `cleaners: {}` while it
    // filled to 1.8 GiB free of 1.8 TiB, and arming that policy would have
    // changed nothing at all.
    //
    // `off` and `report` may legitimately name no cleaner: neither deletes,
    // and both still measure free space and pressure. `enforce` claims it
    // will act, so it has to name something it can act with.
    if map["mode"].as_str() == Some("enforce") && known.is_empty() {
        return Err(verr(
            &cleaners_location,
            "must name at least one cleaner when mode is 'enforce'; an armed \
             policy with no cleaner reports a healthy no-op on a full disk",
        ));
    }
    for (name, cleaner) in cleaners {
        let cleaner_location = format!("{cleaners_location}.{name}");
        let cleaner = cleaner
            .as_object()
            .ok_or_else(|| verr(&cleaner_location, "must be an object"))?;
        const CLEANER_KEYS: [&str; 4] = [
            "allow_missing_upload_proof",
            "keep_newest",
            "min_age_seconds",
            "root",
        ];
        let mut unknown_keys: Vec<&str> = cleaner
            .keys()
            .map(String::as_str)
            .filter(|k| !CLEANER_KEYS.contains(k))
            .collect();
        unknown_keys.sort_unstable();
        if !unknown_keys.is_empty() {
            return Err(verr(
                &cleaner_location,
                &format!("unknown keys {}", py_list_repr(&unknown_keys)),
            ));
        }
        let min_age = cleaner
            .get("min_age_seconds")
            .ok_or_else(|| verr(&cleaner_location, "must contain 'min_age_seconds'"))?;
        // Future cleaner names remain readable by older clients. Only a
        // cleaner this binary implements has a retention floor it can enforce.
        let minimum = crate::providers::local::disk_cleanup::catalogue::cleaner(name.as_str())
            .map_or(0, |entry| entry.min_age_floor_seconds);
        require_int(
            min_age,
            &format!("{cleaner_location}.min_age_seconds"),
            minimum,
            None,
        )?;
        if let Some(proof) = cleaner.get("allow_missing_upload_proof") {
            if !proof.is_boolean() {
                return Err(verr(
                    &format!("{cleaner_location}.allow_missing_upload_proof"),
                    "must be a boolean",
                ));
            }
            if proof.as_bool() == Some(true)
                && name != "weles_recordings"
                && crate::providers::local::disk_cleanup::catalogue::cleaner(name).is_some()
            {
                return Err(verr(
                    &format!("{cleaner_location}.allow_missing_upload_proof"),
                    "only the weles_recordings cleaner accepts missing upload proof",
                ));
            }
        }
        if let Some(root) = cleaner.get("root") {
            if root.as_str().is_none_or(|r| r.trim().is_empty()) {
                return Err(verr(
                    &format!("{cleaner_location}.root"),
                    "must be a non-empty string",
                ));
            }
        }
        // `keep_newest` is the rollback ladder `release_store` leaves per
        // product; it belongs to that cleaner alone and must keep at least
        // one, because a store with zero versions of a product cannot serve
        // the rollback the release loop promises.
        if let Some(keep) = cleaner.get("keep_newest") {
            if name != "release_store" {
                return Err(verr(
                    &format!("{cleaner_location}.keep_newest"),
                    "only the release_store cleaner takes keep_newest",
                ));
            }
            require_int(keep, &format!("{cleaner_location}.keep_newest"), 1, None)?;
        }
    }
    Ok(())
}

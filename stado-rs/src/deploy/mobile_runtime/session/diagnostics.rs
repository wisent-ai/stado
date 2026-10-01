//! Reading the Appium server's own listing: what a driver's version is, and
//! which drivers the server says it cannot host.

/// Every driver the installed server itself calls incompatible with itself.
///
/// Read out of Appium's own startup validation, which writes
/// `Driver "mac2" (package `appium-mac2-driver`) may be incompatible with the
/// current version of Appium (v3.7.0) due to its peer dependency on Appium
/// ^2.4.1`. Using the server's verdict rather than comparing peer ranges here
/// keeps one authority on the question: the server is what will refuse to
/// host the driver, and a second opinion computed in Rust could disagree with
/// it.
pub fn incompatible_drivers(listing: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in listing.split("WARN") {
        if !line.contains("may be incompatible") {
            continue;
        }
        let Some(rest) = line.split_once("Driver \"") else {
            continue;
        };
        let Some((name, _)) = rest.1.split_once('"') else {
            continue;
        };
        if name.is_empty()
            || found
                .iter()
                .any(|(held, _): &(String, String)| held == name)
        {
            continue;
        }
        found.push((
            name.to_string(),
            line.split_whitespace().collect::<Vec<&str>>().join(" "),
        ));
    }
    found
}

/// The version of one installed driver, out of `appium driver list
/// --installed`.
///
/// The listing is decorated differently by Appium 2 and 3 — bullets, colour
/// escapes, an `[installed (npm)]` suffix — and the one token both write
/// identically is `<name>@<version>`. Matched on the exact name so
/// `uiautomator2` is never read out of a line about a different driver, and
/// `None` when the driver is listed without a version rather than a guess.
pub fn installed_driver_version(listing: &str, driver: &str) -> Option<String> {
    let needle = format!("{driver}@");
    let mut rest = listing;
    while let Some(at) = rest.find(&needle) {
        // Reject a suffix match: `test@1` must not answer for `xcuitest@2`.
        let boundary_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '-');
        let tail = &rest[at + needle.len()..];
        let version: String = tail
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect();
        if boundary_ok && !version.is_empty() {
            return Some(version);
        }
        rest = tail;
    }
    None
}

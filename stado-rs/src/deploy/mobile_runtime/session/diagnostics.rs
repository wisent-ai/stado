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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drivers_own_version_is_read_out_of_the_listing() {
        // The shape Appium 3 prints, bullets and suffix included.
        let listing = "- uiautomator2@5.0.1 [installed (npm)] - xcuitest@12.9.1 [installed (npm)]";
        assert_eq!(
            installed_driver_version(listing, "uiautomator2").as_deref(),
            Some("5.0.1")
        );
        assert_eq!(
            installed_driver_version(listing, "xcuitest").as_deref(),
            Some("12.9.1")
        );
        assert_eq!(installed_driver_version(listing, "mac2"), None);
    }

    #[test]
    fn a_driver_name_is_never_read_out_of_a_longer_one() {
        // `xcuitest@12.9.1` must not answer for a driver called `test`, which
        // is the bug a substring search would ship.
        let listing = "- xcuitest@12.9.1 [installed (npm)]";
        assert_eq!(installed_driver_version(listing, "test"), None);
    }

    #[test]
    fn a_listed_driver_with_no_version_is_not_guessed_at() {
        assert_eq!(
            installed_driver_version("- uiautomator2 [installed]", "uiautomator2"),
            None
        );
    }

    #[test]
    fn the_servers_own_incompatibility_warning_names_the_driver() {
        // Verbatim from charless-mac-mini.
        let listing = "WARN Appium Driver \"mac2\" has 1 potential problem: \n\
             WARN Appium   - Driver \"mac2\" (package `appium-mac2-driver`) may be incompatible \
             with the current version of Appium (v3.7.0) due to its peer dependency on Appium \
             ^2.4.1. Please install a compatible version of the driver.\n\
             - mac2@1.20.5 [installed (npm)]\n\
             - uiautomator2@8.5.2 [installed (npm)]";
        let found = incompatible_drivers(listing);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "mac2");
        assert!(found[0].1.contains("peer dependency"));
    }

    #[test]
    fn a_clean_listing_names_no_incompatible_driver() {
        assert!(incompatible_drivers("- uiautomator2@8.5.2 [installed (npm)]").is_empty());
    }
}

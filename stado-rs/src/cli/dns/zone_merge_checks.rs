//! What defends a whole-zone rewrite: which zone a name belongs to, which
//! name the zone refuses, what a merge leaves untouched, what it calls a
//! change, the entities an attribute value carries, and which record types
//! this plane will author at all.

use super::records::write::*;
use super::records::*;
use super::registrar::zone::*;
use super::registrar::*;
use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn record(host: &str, record_type: &str, address: &str) -> Record {
        Record {
            host: host.into(),
            record_type: record_type.into(),
            address: address.into(),
            mx_pref: DEFAULT_MX_PREF.into(),
            ttl: DEFAULT_TTL.into(),
        }
    }

    #[test]
    fn zone_of_takes_the_last_two_labels() {
        let zone = Zone::of("app.preferences.wisent.com").expect("zone");
        assert_eq!(zone.name, "wisent.com");
        assert_eq!(zone.sld, "wisent");
        assert_eq!(zone.tld, "com");
        assert_eq!(
            zone.host_of("app.preferences.wisent.com").expect("host"),
            "app.preferences"
        );
        assert_eq!(zone.host_of("wisent.com").expect("host"), "@");
    }

    #[test]
    fn a_name_outside_the_zone_is_refused() {
        let zone = Zone::parse("wisent.com").expect("zone");
        let error = zone.host_of("preferences.wisent.ai").expect_err("refusal");
        assert!(error.to_string().contains("is not inside zone"), "{error}");
    }

    #[test]
    fn merge_replaces_only_the_named_host_and_type() {
        let before = vec![
            record("@", "A", "76.76.21.21"),
            record("@", "MX", "aspmx.l.google.com"),
            record("preferences", "A", "76.76.21.21"),
        ];
        let (merged, change, replaced) = merge(&before, "preferences", "A", "20.1.2.3", "1800");
        assert_eq!(change, "replaced");
        assert_eq!(replaced.len(), 1);
        assert_eq!(merged.len(), before.len());
        assert!(merged
            .iter()
            .any(|entry| entry.host == "@" && entry.record_type == "MX"));
        assert!(merged
            .iter()
            .any(|entry| entry.host == "preferences" && entry.address == "20.1.2.3"));
    }

    #[test]
    fn merge_reports_an_identical_record_as_unchanged() {
        let before = vec![record("preferences", "A", "20.1.2.3")];
        let (_, change, _) = merge(&before, "preferences", "A", "20.1.2.3", DEFAULT_TTL);
        assert_eq!(change, "unchanged");
    }

    #[test]
    fn merge_creates_a_name_the_zone_does_not_carry() {
        let before = vec![record("@", "A", "76.76.21.21")];
        let (merged, change, replaced) = merge(&before, "app", "A", "20.1.2.3", DEFAULT_TTL);
        assert_eq!(change, "created");
        assert!(replaced.is_empty());
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn attribute_values_are_unescaped() {
        assert_eq!(unescape("v=spf1 &quot;a&quot; &amp; b"), "v=spf1 \"a\" & b");
    }

    #[test]
    fn only_authored_record_types_are_accepted() {
        assert_eq!(normalized_type("a").expect("type"), "A");
        assert!(normalized_type("NS").is_err());
    }
}

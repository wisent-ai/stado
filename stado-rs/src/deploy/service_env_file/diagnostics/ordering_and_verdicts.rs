//! What defends the diagnostics: which assignment wins across both spellings,
//! what a value means once the shell is done with it, which values are
//! endpoints at all, and when a silent port may be called dead.

use super::super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: u32, form: &str, key: &str, value: &str) -> EnvEntry {
        EnvEntry {
            line,
            form: form.to_string(),
            key: key.to_string(),
            value_state: VALUE_SHOWN.to_string(),
            value: value.to_string(),
            chars: value.len() as u32,
        }
    }

    #[test]
    fn the_last_assignment_wins_across_both_spellings() {
        let entries = vec![
            entry(
                1,
                FORM_ASSIGNMENT,
                "WC_SKARBIEC_URL",
                "http://127.0.0.1:8895",
            ),
            entry(2, FORM_ASSIGNMENT, "OTHER", "1"),
            entry(3, FORM_EXPORT, "WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
        ];
        assert_eq!(shadowing(&entries), vec![SHADOWED, EFFECTIVE, EFFECTIVE]);
        assert_eq!(duplicate_keys(&entries), vec!["WC_SKARBIEC_URL"]);
    }

    #[test]
    fn an_unparsable_line_belongs_to_no_key() {
        let mut sourced = entry(4, FORM_UNPARSABLE, "", ". other.env");
        sourced.key = String::new();
        let entries = vec![entry(1, FORM_ASSIGNMENT, "A", "1"), sourced];
        assert_eq!(shadowing(&entries), vec![EFFECTIVE, ""]);
        assert!(duplicate_keys(&entries).is_empty());
    }

    #[test]
    fn quotes_and_trailing_comments_are_not_part_of_the_value() {
        assert_eq!(
            effective_text("\"http://127.0.0.1:8895\""),
            "http://127.0.0.1:8895"
        );
        assert_eq!(effective_text("'8895'"), "8895");
        assert_eq!(
            effective_text("http://127.0.0.1:8895 # live"),
            "http://127.0.0.1:8895"
        );
        assert_eq!(effective_text("  spaced  "), "spaced");
    }

    #[test]
    fn only_a_port_shaped_key_reads_a_bare_integer_as_a_port() {
        assert_eq!(
            declared_endpoint("WELES_API_PORT", "8896"),
            Some(Endpoint {
                port: 8896,
                loopback: true
            })
        );
        assert_eq!(declared_endpoint("WELES_MAX_CONCURRENCY", "4"), None);
    }

    #[test]
    fn loopback_and_remote_urls_are_told_apart() {
        assert_eq!(
            declared_endpoint("WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
            Some(Endpoint {
                port: 8785,
                loopback: true
            })
        );
        assert_eq!(
            declared_endpoint("STADO_API_URL", "https://api.example.com/v1"),
            Some(Endpoint {
                port: 443,
                loopback: false
            })
        );
        assert_eq!(
            declared_endpoint("WELES_HOST", "127.0.0.1:18100"),
            Some(Endpoint {
                port: 18100,
                loopback: true
            })
        );
        assert_eq!(
            declared_endpoint("WELES_IPV6_URL", "http://[::1]:8765/"),
            Some(Endpoint {
                port: 8765,
                loopback: true
            })
        );
        assert_eq!(declared_endpoint("WELES_NOTE", "some prose"), None);
    }

    #[test]
    fn a_dead_endpoint_is_only_dead_when_the_socket_table_was_read() {
        let endpoint = Endpoint {
            port: 8785,
            loopback: true,
        };
        assert_eq!(
            endpoint_verdict(endpoint, &[], LISTENERS_FAILED).0,
            ENDPOINT_UNKNOWN
        );
        assert_eq!(
            endpoint_verdict(endpoint, &[], LISTENERS_READ).0,
            ENDPOINT_DEAD
        );
        let holder = ProcListener {
            address: "127.0.0.1".to_string(),
            port: 8785,
            pid: 4242,
            process: "skarbiec".to_string(),
        };
        let (verdict, holders) = endpoint_verdict(endpoint, &[holder], LISTENERS_READ);
        assert_eq!(verdict, ENDPOINT_LISTENING);
        assert_eq!(holders, vec!["skarbiec (pid 4242)"]);
    }

    #[test]
    fn a_shadowed_endpoint_is_never_the_one_judged() {
        let report = EnvFileReport {
            path: "/home/u/.config/weles/worker.env".to_string(),
            file_state: FILE_READ.to_string(),
            detail: String::new(),
            mode: "600".to_string(),
            owner_only: true,
            bytes: 64,
            entries_state: ENTRIES_READ.to_string(),
            entries: vec![
                entry(
                    1,
                    FORM_ASSIGNMENT,
                    "WC_SKARBIEC_URL",
                    "http://127.0.0.1:8895",
                ),
                entry(2, FORM_EXPORT, "WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
            ],
            entries_seen: 2,
            expected: EXPECT_NOT_ASKED.to_string(),
            listeners_state: LISTENERS_READ.to_string(),
            listeners: vec![ProcListener {
                address: "127.0.0.1".to_string(),
                port: 8895,
                pid: 31909,
                process: "skarbiec".to_string(),
            }],
        };
        let rows = endpoint_rows(&report);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[usize::MIN].port, 8785);
        assert_eq!(rows[usize::MIN].verdict, ENDPOINT_DEAD);
    }
}

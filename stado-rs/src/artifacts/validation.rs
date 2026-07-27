//! Backend-independent artifact manifest validation.
//!
//! Port of `stado/artifacts/validation.py`: the URI scheme allowlist
//! (az/gs/hf/https), embedded-credential and sensitive query-param
//! rejection, and the structural manifest checks.

use regex::Regex;
use url::Url;

use crate::artifacts_models::{ArtifactLocation, ArtifactManifest};

/// Python `_ALLOWED_SCHEMES`.
const ALLOWED_SCHEMES: [&str; 4] = ["az", "gs", "hf", "https"];

fn sensitive_query_key() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(^|[-_])(access[-_]?token|api[-_]?key|credential|password|secret|signature|sig|token)($|[-_])",
        )
        .expect("static regex compiles")
    })
}

/// Python `urlsplit` never fails; WHATWG `Url::parse` rejects relative
/// URIs. This wrapper returns the pieces validation needs, treating
/// unparseable input as "no scheme" so it is reported as `<none>` exactly
/// like a scheme-less Python split.
struct SplitUri {
    scheme: String,
    has_credentials: bool,
    query_keys: Vec<String>,
}

fn split_uri(uri: &str) -> SplitUri {
    match Url::parse(uri) {
        Ok(parsed) => SplitUri {
            // WHATWG lowercases the scheme, as does Python urlsplit.
            scheme: parsed.scheme().to_string(),
            has_credentials: !parsed.username().is_empty() || parsed.password().is_some(),
            query_keys: parsed
                .query_pairs()
                .map(|(key, _)| key.into_owned())
                .collect(),
        },
        Err(_) => {
            // Not an absolute URI. Python would still report a scheme when
            // the text before ':' looks like one (`[A-Za-z][A-Za-z0-9+.-]*`),
            // e.g. `s3:bucket` parses fine in WHATWG too, so the only URIs
            // landing here have no usable scheme.
            SplitUri {
                scheme: String::new(),
                has_credentials: false,
                query_keys: Vec::new(),
            }
        }
    }
}

fn is_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn validate_location(location: &ArtifactLocation, prefix: &str, issues: &mut Vec<String>) {
    let parsed = split_uri(&location.uri);
    if !ALLOWED_SCHEMES.contains(&parsed.scheme.as_str()) {
        let scheme = if parsed.scheme.is_empty() {
            "<none>"
        } else {
            &parsed.scheme
        };
        issues.push(format!("{prefix}.uri uses unsupported scheme: {scheme}"));
    }
    if parsed.has_credentials {
        issues.push(format!("{prefix}.uri must not embed credentials"));
    }
    for key in &parsed.query_keys {
        if sensitive_query_key().is_match(key) {
            issues.push(format!(
                "{prefix}.uri contains sensitive query field: {key}"
            ));
        }
    }
    if !location.sha256.is_empty() && !is_hex_sha256(&location.sha256.to_lowercase()) {
        issues.push(format!(
            "{prefix}.sha256 must contain 64 hexadecimal characters"
        ));
    }
    if location.size_bytes.is_some_and(|n| n < 0) {
        issues.push(format!("{prefix}.size_bytes cannot be negative"));
    }
    if location.file_count.is_some_and(|n| n < 0) {
        issues.push(format!("{prefix}.file_count cannot be negative"));
    }
}

/// Python `validate_manifest`: the ordered tuple of issue strings (empty
/// when the manifest is valid).
pub fn validate_manifest(manifest: &ArtifactManifest) -> Vec<String> {
    let mut issues: Vec<String> = Vec::new();
    if manifest.schema_version != 1 {
        issues.push(format!(
            "unsupported schema_version: {}",
            manifest.schema_version
        ));
    }
    if manifest.title.trim().is_empty() {
        issues.push("title is required".to_string());
    }
    if manifest.locations.is_empty() {
        issues.push("at least one location is required".to_string());
    }
    if !manifest.dependencies.is_empty() && manifest.dependencies.contains(&manifest.ref_) {
        issues.push("artifact cannot depend on itself".to_string());
    }

    let mut primary = 0usize;
    for (index, location) in manifest.locations.iter().enumerate() {
        if location.role == "primary" {
            primary += 1;
        }
        validate_location(location, &format!("locations[{index}]"), &mut issues);
    }
    if primary != 1 {
        issues.push("exactly one primary location is required".to_string());
    }

    for (key, value) in &manifest.labels {
        if key.is_empty() || key.chars().count() > 128 || value.chars().count() > 512 {
            issues.push(
                "labels must have non-empty keys <=128 and values <=512 characters".to_string(),
            );
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts_models::{ArtifactLocation, ArtifactRef};

    fn manifest_with_uris(uris: &[&str]) -> ArtifactManifest {
        let mut manifest = ArtifactManifest::new(
            ArtifactRef::new("dataset", "wisent", "demo", "v1").unwrap(),
            "Demo",
        );
        manifest.locations = uris
            .iter()
            .enumerate()
            .map(|(i, uri)| ArtifactLocation {
                role: if i == 0 { "primary" } else { "mirror" }.to_string(),
                uri: uri.to_string(),
                storage: "test".to_string(),
                immutable_revision: String::new(),
                sha256: String::new(),
                size_bytes: None,
                file_count: None,
            })
            .collect();
        manifest
    }

    fn valid_manifest() -> ArtifactManifest {
        manifest_with_uris(&["gs://stado/artifacts/demo"])
    }

    #[test]
    fn valid_manifest_has_no_issues() {
        assert_eq!(validate_manifest(&valid_manifest()), Vec::<String>::new());
        // Every allowed scheme passes the scheme check.
        for scheme in ["az", "gs", "hf", "https"] {
            let m = manifest_with_uris(&[format!("{scheme}://bucket/path").as_str()]);
            assert_eq!(
                validate_manifest(&m),
                Vec::<String>::new(),
                "scheme {scheme}"
            );
        }
    }

    #[test]
    fn unsupported_and_missing_schemes_are_rejected() {
        let m = manifest_with_uris(&["s3://bucket/demo"]);
        assert_eq!(
            validate_manifest(&m),
            vec!["locations[0].uri uses unsupported scheme: s3".to_string()]
        );
        let m = manifest_with_uris(&["relative/path"]);
        assert_eq!(
            validate_manifest(&m),
            vec!["locations[0].uri uses unsupported scheme: <none>".to_string()]
        );
    }

    #[test]
    fn embedded_credentials_are_rejected() {
        let m = manifest_with_uris(&["https://user:secret@example.com/data"]);
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(&"locations[0].uri must not embed credentials".to_string()),
            "{issues:?}"
        );
        let m = manifest_with_uris(&["https://user@example.com/data"]);
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(&"locations[0].uri must not embed credentials".to_string()),
            "{issues:?}"
        );
    }

    #[test]
    fn sensitive_query_fields_are_rejected() {
        // The full sensitive list from validation.py's _SENSITIVE_QUERY_KEY.
        for key in [
            "token",
            "access_token",
            "access-token",
            "api_key",
            "apikey",
            "credential",
            "password",
            "secret",
            "signature",
            "sig",
            "x-goog-signature",
        ] {
            let m = manifest_with_uris(&[format!("https://example.com/d?{key}=abc").as_str()]);
            let issues = validate_manifest(&m);
            assert!(
                issues.iter().any(|i| i.contains("sensitive query field")),
                "key {key} not flagged: {issues:?}"
            );
        }
        // Non-sensitive keys (substring lookalikes) must pass.
        for key in ["sigmoid", "tokens", "format", "tokenize"] {
            let m = manifest_with_uris(&[format!("https://example.com/d?{key}=abc").as_str()]);
            let issues = validate_manifest(&m);
            assert!(
                !issues.iter().any(|i| i.contains("sensitive query field")),
                "key {key} wrongly flagged: {issues:?}"
            );
        }
    }

    #[test]
    fn structural_checks() {
        // No locations.
        let mut m = ArtifactManifest::new(
            ArtifactRef::new("dataset", "wisent", "demo", "v1").unwrap(),
            "Demo",
        );
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(&"at least one location is required".to_string()),
            "{issues:?}"
        );
        assert!(
            issues.contains(&"exactly one primary location is required".to_string()),
            "{issues:?}"
        );

        // Two primaries.
        m = manifest_with_uris(&["gs://a/x", "gs://a/y"]);
        m.locations[1].role = "primary".into();
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(&"exactly one primary location is required".to_string()),
            "{issues:?}"
        );

        // Self-dependency.
        m = valid_manifest();
        m.dependencies.push(m.ref_.clone());
        assert!(validate_manifest(&m).contains(&"artifact cannot depend on itself".to_string()));

        // Bad sha256 / negative counts.
        m = valid_manifest();
        m.locations[0].sha256 = "deadbeef".into();
        m.locations[0].size_bytes = Some(-1);
        m.locations[0].file_count = Some(-2);
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(
                &"locations[0].sha256 must contain 64 hexadecimal characters".to_string()
            ),
            "{issues:?}"
        );
        assert!(
            issues.contains(&"locations[0].size_bytes cannot be negative".to_string()),
            "{issues:?}"
        );
        assert!(
            issues.contains(&"locations[0].file_count cannot be negative".to_string()),
            "{issues:?}"
        );
        // Uppercase hex is accepted (Python lowercases before matching).
        m = valid_manifest();
        m.locations[0].sha256 = "A".repeat(64);
        assert_eq!(validate_manifest(&m), Vec::<String>::new());

        // schema_version / title / labels.
        m = valid_manifest();
        m.schema_version = 2;
        m.title = "  ".into();
        m.labels.insert("x".repeat(129), "v".into());
        let issues = validate_manifest(&m);
        assert!(
            issues.contains(&"unsupported schema_version: 2".to_string()),
            "{issues:?}"
        );
        assert!(
            issues.contains(&"title is required".to_string()),
            "{issues:?}"
        );
        assert!(
            issues.contains(
                &"labels must have non-empty keys <=128 and values <=512 characters".to_string()
            ),
            "{issues:?}"
        );
    }
}

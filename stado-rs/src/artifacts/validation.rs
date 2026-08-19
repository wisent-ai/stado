//! Backend-independent artifact manifest validation.
//!
//! Artifact locations accept the provider-neutral `stado://` namespace,
//! provider-native object locators used by the multi-cloud lifecycle, and
//! immutable external sources. Credentials remain forbidden in every URI.

use regex::Regex;
use url::Url;

use crate::artifacts_models::{ArtifactLocation, ArtifactManifest};

/// Schemes emitted by the artifact and storage providers.
const ALLOWED_SCHEMES: &[&str] = &["stado", "az", "gs", "s3", "hf", "https"];

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


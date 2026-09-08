//! What a fresh declaration says, the Sunshine artifact this build pins for
//! the host's distribution, and the declaration echoed back beside a report.

use serde_json::{json, Value};

use crate::stream::schema::DisplayStream;

/// The declaration a fresh `stream declare` writes.
///
/// `release` is the host's own `ID VERSION_ID` from the probe, because the
/// artifact that installs is a property of the distribution and not of this
/// build: the 26.04 package wants `libc6 >= 2.43` and `libicu78`, and on the
/// fleet's Ubuntu 25.10 host apt answered "[no choices]" for exactly that
/// reason.
pub fn default_declaration(
    resolution: &str,
    refresh_hz: u16,
    gpu_uuid: Option<String>,
    library_dir: &str,
    steam: bool,
    release: &str,
) -> Result<DisplayStream, String> {
    Ok(DisplayStream {
        enabled: true,
        session: crate::stream::schema::SESSION_X11.to_string(),
        resolution: resolution.to_string(),
        refresh_hz,
        gpu_uuid,
        library_dir: library_dir.to_string(),
        sunshine: pinned_sunshine_for(release)?,
        steam,
    })
}

/// The Sunshine release this build pins, and which published artifact suits a
/// given distribution. Nothing here resolves "latest", and every digest was
/// measured from the published asset (the project's release API reports the same
/// values under `assets[].digest`).
pub const SUNSHINE_VERSION: &str = "v2026.516.143833";

const SUNSHINE_ARTIFACTS: &[(&str, &str, &str)] = &[
    // release prefix, asset name, sha256
    ("ubuntu 22.04", "sunshine-ubuntu-22.04-amd64.deb", ""),
    (
        "ubuntu 24.04",
        "sunshine-ubuntu-24.04-amd64.deb",
        "6df8900f23c9c056252eea51639507b8239a1d1241308ab8923cb402b0ca653b",
    ),
    (
        // 25.10 carries libicu76 and glibc 2.42: the Debian trixie build is the
        // published artifact whose dependencies that satisfies, while both the
        // 24.04 (libicu74) and 26.04 (libicu78, glibc 2.43) packages do not.
        "ubuntu 25.10",
        "sunshine-debian-trixie-amd64.deb",
        "b9b65f2be93b3e30be0710a940a616b1381da5bc6d858dce33bc0094d7fd4131",
    ),
    (
        "ubuntu 26.04",
        "sunshine-ubuntu-26.04-amd64.deb",
        "c7e5452f8cf2609dffbdeda63ca3be7ee45f91505dc496844d65924817cb2517",
    ),
    (
        "debian 13",
        "sunshine-debian-trixie-amd64.deb",
        "b9b65f2be93b3e30be0710a940a616b1381da5bc6d858dce33bc0094d7fd4131",
    ),
];

/// The pinned artifact for one distribution, or a refusal that names what is
/// known. A guess here would be a host that installs something its libraries
/// cannot load.
pub fn pinned_sunshine_for(
    release: &str,
) -> Result<crate::stream::schema::SunshineRelease, String> {
    let normalised = release.trim().to_lowercase();
    let found = SUNSHINE_ARTIFACTS
        .iter()
        .find(|(prefix, _, digest)| normalised.starts_with(prefix) && !digest.is_empty());
    let Some((_, asset, digest)) = found else {
        let known: Vec<&str> = SUNSHINE_ARTIFACTS
            .iter()
            .filter(|(_, _, digest)| !digest.is_empty())
            .map(|(prefix, _, _)| *prefix)
            .collect();
        return Err(format!(
            "no Sunshine artifact is pinned for {release:?}; known: {}. Pass --sunshine-url and \
             --sunshine-sha256 for a measured artifact instead of guessing",
            known.join(", ")
        ));
    };
    Ok(crate::stream::schema::SunshineRelease {
        version: SUNSHINE_VERSION.to_string(),
        deb_url: format!(
            "https://github.com/LizardByte/Sunshine/releases/download/{SUNSHINE_VERSION}/{asset}"
        ),
        deb_sha256: (*digest).to_string(),
    })
}

/// Report shaped for `--json`, with the declaration echoed beside the host's
/// answer so a reader sees both halves of the comparison.
pub fn with_declaration(mut report: Value, declaration: &DisplayStream) -> Value {
    if let Some(map) = report.as_object_mut() {
        map.insert(
            "declaration".to_string(),
            serde_json::to_value(declaration).unwrap_or(json!(null)),
        );
    }
    report
}

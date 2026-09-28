//! The desktop update channel, served from Stado's own publication.
//!
//! A desktop release's build stages `<Display>.zip`, its Sparkle signature and
//! `appcast.xml` into the release archive Stado signs and publishes at
//! `stado://releases/<product>/<version>/darwin-arm64/release.tar.gz`. These
//! two public routes serve an installed app from those bytes:
//!
//! - `GET /api/release/appcast?product=P` answers the `appcast.xml` of P's
//!   newest installable stable release: the newest run of P on the stable
//!   channel whose state says its channel pointer moved
//!   ([`crate::release_pipeline::ReleaseRunState::published`]) and whose
//!   darwin-arm64 platform is published.
//! - `GET /api/release/sparkle?product=P&version=V&file=NAME.zip` answers one
//!   `.zip` member of exactly that installable stable release of P at V.
//!
//! Each answer is read from the published archive after its SHA-256 matches
//! the digest the run recorded when Stado published it. Nothing else of an
//! archive is served, and no run that failed, was superseded or is still in
//! flight is ever the feed.
//!
//! This replaced a delivery that uploaded the same three files to a separate
//! host with its own token; that host never resolved and the token items were
//! never minted, so no desktop app had a feed.

use std::io::Read;

use serde_json::json;

use crate::dashboard::listener::{
    http_status, parse_qs, query_value, send_json, Dashboard, Response,
};
use crate::dashboard::DashboardError;
use crate::release_pipeline::{PipelineChannel, PlatformRunState, ReleaseRun};

/// The route that answers a product's current appcast.
pub(crate) const APPCAST_PATH: &str = "/api/release/appcast";
/// The route that answers one update archive of an exact version.
pub(crate) const ARCHIVE_PATH: &str = "/api/release/sparkle";

/// The one platform a Sparkle application is published for.
const PLATFORM: &str = "darwin-arm64";
/// The appcast member every desktop release stages.
const APPCAST_MEMBER: &str = "appcast.xml";
/// The only members the archive route serves.
const UPDATE_SUFFIX: &str = ".zip";
/// Where release runs are stored, one `<run id>/run.json` each.
const RUNS_PREFIX: &str = "runs/release-pipeline/";
const RUN_LEAF: &str = "/run.json";
/// `list_paths` reads every path under the prefix when asked for none of
/// the oldest in particular.
const EVERY_PATH: usize = 0;

const APPCAST_TYPE: &str = "application/xml";
const ARCHIVE_TYPE: &str = "application/zip";

/// A query parameter present exactly once and not empty.
fn single(values: &[(String, String)], name: &str) -> Option<String> {
    let mut named = values.iter().filter(|(key, _)| key == name);
    match (named.next(), named.next()) {
        (Some(_), None) => query_value(values, name).filter(|value| !value.is_empty()),
        _ => None,
    }
}

fn refused(status: &str, detail: String) -> Response {
    send_json(http_status(status), &json!({ "error": detail }))
}

/// An update archive's member name: `<something>.zip`, one path component.
fn update_member(file: &str) -> bool {
    file.ends_with(UPDATE_SUFFIX)
        && file.len() > UPDATE_SUFFIX.len()
        && !file.starts_with('.')
        && !file.contains(['/', '\\'])
}

/// Whether a stored run is an installable stable release of `product`, and
/// at `version` when one is named.
fn installable(run: &ReleaseRun, product: &str, version: Option<&str>) -> bool {
    run.product == product
        && version.is_none_or(|wanted| run.version == wanted)
        && run.channel == PipelineChannel::Stable
        && run.state.published()
        && run
            .platforms
            .get(PLATFORM)
            .is_some_and(|platform| platform.state == PlatformRunState::Published)
}

/// The regular file whose last path component is `name` in a gzipped tar,
/// as the release build staged it.
fn member(archive: &[u8], name: &str) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut bundle = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in bundle.entries()? {
        let mut entry = entry?;
        let named = entry.path()?.file_name().and_then(|leaf| leaf.to_str()) == Some(name);
        if named && entry.header().entry_type().is_file() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

impl Dashboard {
    pub(crate) async fn get_appcast(&self, query: &str) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let Some(product) = single(&values, "product") else {
            return Ok(refused(
                "400",
                "the query must name exactly one product".to_string(),
            ));
        };
        self.serve_member(&product, None, APPCAST_MEMBER, APPCAST_TYPE)
            .await
    }

    pub(crate) async fn get_sparkle_archive(
        &self,
        query: &str,
    ) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let (Some(product), Some(version), Some(file)) = (
            single(&values, "product"),
            single(&values, "version"),
            single(&values, "file"),
        ) else {
            return Ok(refused(
                "400",
                "the query must name exactly one product, version and file".to_string(),
            ));
        };
        if !update_member(&file) {
            return Ok(refused(
                "403",
                format!("only a release's {UPDATE_SUFFIX} update archive is served, not {file:?}"),
            ));
        }
        self.serve_member(&product, Some(&version), &file, ARCHIVE_TYPE)
            .await
    }

    /// One member of the newest installable stable release of `product`
    /// (at `version` when named), read from its digest-checked archive.
    async fn serve_member(
        &self,
        product: &str,
        version: Option<&str>,
        name: &str,
        content_type: &str,
    ) -> Result<Response, DashboardError> {
        let Some(run) = self.newest_installable(product, version).await? else {
            return Ok(send_json(
                http_status("404"),
                &json!({
                    "state": "absent",
                    "product": product,
                    "version": version,
                    "detail": "no installable stable darwin-arm64 release of this product is published",
                }),
            ));
        };
        let base = crate::release_control::release_base(&run.product, &run.version, PLATFORM)
            .map_err(DashboardError::Other)?;
        let uri = format!("{base}/{}", crate::release_control::RELEASE_ARCHIVE_NAME);
        let recorded = run
            .platforms
            .get(PLATFORM)
            .and_then(|platform| platform.artifact_sha256.clone());
        let Some(archive) = self.store.download_release(&uri).await? else {
            return Ok(refused(
                "503",
                format!(
                    "release run {} is published but {uri} is absent",
                    run.run_id
                ),
            ));
        };
        let digest = crate::release_control::sha256_bytes(&archive);
        if recorded.as_deref() != Some(digest.as_str()) {
            return Ok(refused(
                "503",
                format!(
                    "{uri} has SHA-256 {digest}, but release run {} recorded {}",
                    run.run_id,
                    recorded.as_deref().unwrap_or("no digest")
                ),
            ));
        }
        let Some(bytes) = member(&archive, name)? else {
            return Ok(send_json(
                http_status("404"),
                &json!({"state": "absent", "uri": uri, "member": name}),
            ));
        };
        Ok(Response::new_with_headers(
            http_status("200"),
            "OK",
            content_type,
            &bytes,
            &[(
                "X-Stado-Release",
                format!("{} {}", run.product, run.version),
            )],
        ))
    }

    /// The newest installable stable run of `product`, by creation time and
    /// then run id, the order every other reader of these runs uses.
    async fn newest_installable(
        &self,
        product: &str,
        version: Option<&str>,
    ) -> Result<Option<ReleaseRun>, DashboardError> {
        let mut newest: Option<ReleaseRun> = None;
        for path in self.store.list_paths(RUNS_PREFIX, EVERY_PATH).await? {
            if !path.ends_with(RUN_LEAF) {
                continue;
            }
            let Some(text) = self.store.download_text(&path).await? else {
                continue;
            };
            let run: ReleaseRun = serde_json::from_str(&text).map_err(|error| {
                DashboardError::Other(format!("invalid release run {path}: {error}"))
            })?;
            if !installable(&run, product, version) {
                continue;
            }
            let newer = newest.as_ref().is_none_or(|current| {
                (run.created_at.as_str(), run.run_id.as_str())
                    > (current.created_at.as_str(), current.run_id.as_str())
            });
            if newer {
                newest = Some(run);
            }
        }
        Ok(newest)
    }
}

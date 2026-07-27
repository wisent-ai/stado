//! Cross-provider GPU catalog enumerator + write-side fan-outs.
//!
//! Port of `stado/scheduler/dispatch/quota_skus.py`: `provider_catalog`
//! returns the full list of GPU-related SKUs/families the provider
//! supports along with the current per-region limit on file for our
//! project. Backs `stado quota catalog` (read-side enumeration) and
//! `stado quota request-all` (bulk fan-out of CreateQuotaPreference
//! across every enumerated family × every configured region), plus
//! `gcp_request_status` (backs `stado quota requests`).
//!
//! GCP path: the Python code uses google-cloud-quotas `list_quota_infos`;
//! this port calls the Cloud Quotas REST API directly
//! (`GET https://cloudquotas.googleapis.com/v1/projects/{p}/locations/
//! global/services/compute.googleapis.com/quotaInfos`) with gcp_auth,
//! filtered to GPU-related quota_ids:
//!   - NVIDIA-{FAMILY}-GPUS-per-project-region   (legacy per-family
//!     quotas, one per GPU model, dimensioned by region)
//!   - GPUS-PER-GPU-FAMILY-per-project-region    (newer unified quota,
//!     dimensioned by gpu_family + region)
//!
//! The newer GPUS-PER-GPU-FAMILY quota is the right submission target;
//! the legacy per-family quotas are kept for read-side completeness so a
//! catalog dump shows everything Google tracks.
//!
//! Azure path uses `az vm list-skus` to enumerate Compute GPU VM families
//! in the subscription, as a subprocess like Python.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::LazyLock;

use serde_json::{json, Value};

/// Cloud Quotas API v1 base.
pub const CLOUD_QUOTAS_BASE: &str = "https://cloudquotas.googleapis.com/v1";
/// OAuth scope for the Cloud Quotas REST read.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Python's GPU-quota filter: `re.search(r"NVIDIA-[A-Z0-9_-]+-GPUS", qid)`.
fn legacy_gpu_quota_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"NVIDIA-[A-Z0-9_-]+-GPUS").expect("static regex compiles")
    });
    &RE
}

/// Catalog fetch error.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// Python: ADC lookup failure at CloudQuotasClient construction.
    #[error("no GCP credentials found for the Cloud Quotas API: {0}")]
    Auth(String),
    /// Transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Non-2xx response; message carries status + body head.
    #[error("{0}")]
    Api(String),
}

/// Bearer-authenticated Cloud Quotas REST v1 client. Cheap to clone.
#[derive(Clone)]
pub struct CloudQuotasClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    project: String,
    base_url: String,
    auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
}

impl CloudQuotasClient {
    /// Bind to the public Cloud Quotas API, resolving GCP credentials.
    pub async fn new(project: &str) -> Result<Self, CatalogError> {
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|err| CatalogError::Auth(err.to_string()))?;
        Ok(Self::assemble(project, CLOUD_QUOTAS_BASE, Some(auth)))
    }

    /// Bind to an explicit base URL without credentials (loopback mocks).
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str, project: &str) -> Self {
        Self::assemble(project, base_url, None)
    }

    fn assemble(
        project: &str,
        base_url: &str,
        auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
    ) -> Self {
        CloudQuotasClient {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                project: project.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
            }),
        }
    }

    /// The project this client reads quota infos for.
    pub fn project(&self) -> &str {
        &self.inner.project
    }

    /// Shared authenticated JSON request. Non-2xx lifts to
    /// [`CatalogError::Api`] carrying status + body head (so the Python
    /// ALREADY_EXISTS substring checks keep working on the message).
    async fn send_json(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        desc: &str,
    ) -> Result<Value, CatalogError> {
        let mut request = self
            .inner
            .http
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(auth) = &self.inner.auth {
            let token = auth
                .token(&[CLOUD_PLATFORM_SCOPE])
                .await
                .map_err(|err| CatalogError::Auth(err.to_string()))?;
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.as_str()),
            );
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            let head: String = text.chars().take(280).collect();
            return Err(CatalogError::Api(format!(
                "Cloud Quotas {desc} -> HTTP {status}: {head}"
            )));
        }
        let text = response.text().await.unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|err| CatalogError::Api(format!("Cloud Quotas {desc} -> invalid JSON: {err}")))
    }

    /// CreateQuotaPreference: POST `{base}/projects/{p}/locations/global/
    /// quotaPreferences?quotaPreferenceId={id}` (Python
    /// `client.create_quota_preference`).
    pub async fn create_quota_preference(
        &self,
        quota_preference_id: &str,
        body: &Value,
    ) -> Result<Value, CatalogError> {
        let url = format!(
            "{}/projects/{}/locations/global/quotaPreferences?quotaPreferenceId={}",
            self.inner.base_url,
            self.inner.project,
            crate::queue::gcs::percent_encode(quota_preference_id)
        );
        self.send_json(
            reqwest::Method::POST,
            &url,
            Some(body),
            "create_quota_preference",
        )
        .await
    }

    /// UpdateQuotaPreference: PATCH `{base}/projects/{p}/locations/global/
    /// quotaPreferences/{id}` (Python `client.update_quota_preference`).
    pub async fn update_quota_preference(
        &self,
        quota_preference_id: &str,
        body: &Value,
    ) -> Result<Value, CatalogError> {
        let url = format!(
            "{}/projects/{}/locations/global/quotaPreferences/{}",
            self.inner.base_url,
            self.inner.project,
            crate::queue::gcs::percent_encode(quota_preference_id)
        );
        self.send_json(
            reqwest::Method::PATCH,
            &url,
            Some(body),
            "update_quota_preference",
        )
        .await
    }

    /// `list_quota_preferences` over REST with pageToken pagination:
    /// GET `{base}/projects/{p}/locations/global/quotaPreferences`.
    pub async fn list_quota_preferences(&self) -> Result<Vec<Value>, CatalogError> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/projects/{}/locations/global/quotaPreferences",
                self.inner.base_url, self.inner.project
            );
            if let Some(token) = &page_token {
                url.push_str(&format!(
                    "?pageToken={}",
                    crate::queue::gcs::percent_encode(token)
                ));
            }
            let page = self
                .send_json(reqwest::Method::GET, &url, None, "list_quota_preferences")
                .await?;
            if let Some(prefs) = page.get("quotaPreferences").and_then(Value::as_array) {
                out.extend(prefs.iter().cloned());
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) if !token.is_empty() => page_token = Some(token.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// `list_quota_infos` over REST with pageToken pagination:
    /// `GET {base}/projects/{p}/locations/global/services/
    /// compute.googleapis.com/quotaInfos`.
    pub async fn list_quota_infos(&self) -> Result<Vec<Value>, CatalogError> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/projects/{}/locations/global/services/compute.googleapis.com/quotaInfos",
                self.inner.base_url, self.inner.project
            );
            if let Some(token) = &page_token {
                url.push_str(&format!(
                    "?pageToken={}",
                    crate::queue::gcs::percent_encode(token)
                ));
            }
            let mut request = self
                .inner
                .http
                .get(&url)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(auth) = &self.inner.auth {
                let token = auth
                    .token(&[CLOUD_PLATFORM_SCOPE])
                    .await
                    .map_err(|err| CatalogError::Auth(err.to_string()))?;
                request = request.header(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", token.as_str()),
                );
            }
            let response = request.send().await?;
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();
                let head: String = text.chars().take(280).collect();
                return Err(CatalogError::Api(format!(
                    "Cloud Quotas list_quota_infos -> HTTP {status}: {head}"
                )));
            }
            let page: Value = response.json().await.unwrap_or(Value::Null);
            if let Some(infos) = page.get("quotaInfos").and_then(Value::as_array) {
                out.extend(infos.iter().cloned());
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) if !token.is_empty() => page_token = Some(token.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }
}

/// The GCP project the catalog read targets. Python quota_skus.py resolves
/// `os.environ.get("GCP_PROJECT", "wisent-480400")` — env only, NOT
/// config.PROJECT. Kept env-only for parity.
pub(crate) fn gcp_project_env() -> String {
    std::env::var("GCP_PROJECT").unwrap_or_else(|_| "wisent-480400".to_string())
}

/// Enumerate every GPU-related compute.googleapis.com QuotaInfo in the
/// project, one entry per (quota_id, region) with its current limit
/// (Python `_gcp_catalog`). The limit comes from
/// QuotaInfo.dimensionsInfos: each DimensionsInfo carries an
/// applicableLocations list + a details.value field that is the current
/// per-region cap.
///
/// Note: no hardcoded family list. Rows whose quota_id is the unified
/// GPUS-PER-GPU-FAMILY-per-project-region quota carry a populated
/// `gpu_family` dimension that is the ground truth for what families
/// Google currently models in this project. Anything else would
/// reintroduce the "hardcoded list drifts from reality" problem.
pub async fn gcp_catalog(client: &CloudQuotasClient) -> Result<Vec<Value>, CatalogError> {
    let mut out = Vec::new();
    for info in client.list_quota_infos().await? {
        let qid = info.get("quotaId").and_then(Value::as_str).unwrap_or("");
        let is_gpu_family = qid.contains("GPUS-PER-GPU-FAMILY");
        let is_legacy_gpu = legacy_gpu_quota_re().is_match(qid);
        if !(is_gpu_family || is_legacy_gpu) {
            continue;
        }
        // metric_display_name or metric (empty display name falls back).
        let metric = info
            .get("metricDisplayName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| info.get("metric").and_then(Value::as_str))
            .unwrap_or("");
        let empty_dims = Vec::new();
        let dims_infos = info
            .get("dimensionsInfos")
            .and_then(Value::as_array)
            .unwrap_or(&empty_dims);
        for di in dims_infos {
            let locs: Vec<&str> = di
                .get("applicableLocations")
                .and_then(Value::as_array)
                .map(|locs| locs.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            // details.value is an int64 (string-encoded per the Google
            // JSON convention); int(value) in Python.
            let limit = di
                .get("details")
                .and_then(|d| d.get("value"))
                .and_then(|v| match v {
                    Value::String(s) => s.parse::<i64>().ok(),
                    Value::Number(n) => n.as_i64(),
                    _ => None,
                });
            let gpu_family = di
                .get("dimensions")
                .and_then(|d| d.get("gpu_family"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // Python `for loc in locs or ["global"]`.
            let locations: Vec<&str> = if locs.is_empty() {
                vec!["global"]
            } else {
                locs
            };
            for loc in locations {
                out.push(json!({
                    "provider": "gcp",
                    "quota_id": qid,
                    "metric": metric,
                    "gpu_family": gpu_family,
                    "region": loc,
                    "limit": limit,
                }));
            }
        }
    }
    Ok(out)
}

/// The pure SKU-table half of Python `_azure_catalog`: keep families
/// containing NC/ND/NV/GPU, one row per (family, location). Split out for
/// tests.
pub fn azure_rows_from_skus(skus: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for sku in skus {
        let family = sku.get("family").and_then(Value::as_str).unwrap_or("");
        if !["NC", "ND", "NV", "GPU"].iter().any(|t| family.contains(t)) {
            continue;
        }
        let name = sku.get("name").and_then(Value::as_str).unwrap_or("");
        // Mark each (family, location) row once; the SKU table has many
        // SKUs per family, so we dedupe at print/aggregate time.
        if let Some(locations) = sku.get("locations").and_then(Value::as_array) {
            for loc in locations.iter().filter_map(Value::as_str) {
                out.push(json!({
                    "provider": "azure",
                    "family": family,
                    "sku": name,
                    "location": loc,
                }));
            }
        }
    }
    out
}

/// Enumerate Azure Compute GPU VM families across every location available to
/// the subscription through ARM. Authentication uses managed identity or the
/// `stado-azure` Skarbiec item; Azure CLI is not consulted.
pub async fn azure_catalog() -> Vec<Value> {
    let subscription = crate::config::azure_subscription_id();
    if subscription.is_empty() {
        return vec![json!({
            "provider": "azure",
            "ok": false,
            "error": "AZURE_SUBSCRIPTION_ID is required",
        })];
    }
    let http = reqwest::Client::new();
    let token = match crate::azure_token::identity_bearer_token(
        &http,
        "https://management.azure.com/.default",
        "https://management.azure.com",
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            return vec![json!({
                "provider": "azure",
                "ok": false,
                "error": err.to_string(),
            })];
        }
    };
    let mut next = Some(format!(
        "https://management.azure.com/subscriptions/{subscription}/providers/Microsoft.Compute/skus?api-version=2021-07-01"
    ));
    let mut skus = Vec::new();
    while let Some(url) = next.take() {
        let response = match http.get(url).bearer_auth(&token).send().await {
            Ok(response) => response,
            Err(err) => {
                return vec![json!({
                    "provider": "azure",
                    "ok": false,
                    "error": err.to_string(),
                })];
            }
        };
        let status = response.status();
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(err) => {
                return vec![json!({
                    "provider": "azure",
                    "ok": false,
                    "error": err.to_string(),
                })];
            }
        };
        if !status.is_success() {
            return vec![json!({
                "provider": "azure",
                "ok": false,
                "error": format!("Azure Compute SKU list returned HTTP {status}: {body}"),
            })];
        }
        skus.extend(
            body.get("value")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
        next = body
            .get("nextLink")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    azure_rows_from_skus(&skus)
}

/// Return the full GPU catalog for `provider` (gcp | azure) — Python
/// `provider_catalog`. `gcp_client` is injectable for tests; `None`
/// resolves a live client for the gcp arm.
pub async fn provider_catalog(
    provider: &str,
    gcp_client: Option<&CloudQuotasClient>,
) -> Result<Vec<Value>, CatalogError> {
    match provider {
        "gcp" => {
            let owned;
            let client = match gcp_client {
                Some(client) => client,
                None => {
                    owned = CloudQuotasClient::new(&gcp_project_env()).await?;
                    &owned
                }
            };
            gcp_catalog(client).await
        }
        "azure" => Ok(azure_catalog().await),
        other => Ok(vec![json!({
            "provider": other,
            "ok": false,
            "error": "no catalog impl for this provider",
        })]),
    }
}

/// provider_name -> list of catalog rows (Python `all_catalogs`).
/// Deviation: the Rust map is BTreeMap-ordered (alphabetical) where the
/// Python dict preserves the input `providers` order; the CLI's --json
/// output sorts keys anyway.
pub async fn all_catalogs(
    providers: &[String],
    gcp_client: Option<&CloudQuotasClient>,
) -> Result<BTreeMap<String, Vec<Value>>, CatalogError> {
    let mut out = BTreeMap::new();
    for provider in providers {
        out.insert(
            provider.clone(),
            provider_catalog(provider, gcp_client).await?,
        );
    }
    Ok(out)
}

/// Google int64 JSON convention: string-encoded, but tolerate numbers.
fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// One row per QuotaPreference (Python `gcp_request_status`). Buckets
/// stateDetail into a state field (approved/partially_approved/denied/
/// reconciling/unknown).
pub async fn gcp_request_status(client: &CloudQuotasClient) -> Result<Vec<Value>, CatalogError> {
    let mut out = Vec::new();
    for pref in client.list_quota_preferences().await? {
        let config = pref.get("quotaConfig").cloned().unwrap_or(Value::Null);
        let sd = config
            .get("stateDetail")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let reconciling = pref
            .get("reconciling")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = if reconciling {
            "reconciling"
        } else if sd.contains("partially approved") {
            "partially_approved"
        } else if sd.contains("approved") {
            "approved"
        } else if sd.contains("denied") {
            "denied"
        } else {
            "unknown"
        };
        let dims = pref.get("dimensions").cloned().unwrap_or(json!({}));
        // Python `qc.granted_value if qc and qc.granted_value else None`:
        // a missing/zero granted value is null in the row.
        let granted = json_i64(config.get("grantedValue")).filter(|v| *v != 0);
        out.push(json!({
            "name": pref.get("name").and_then(Value::as_str).unwrap_or(""),
            "quota_id": pref.get("quotaId").and_then(Value::as_str).unwrap_or(""),
            "gpu_family": dims.get("gpu_family").and_then(Value::as_str).unwrap_or(""),
            "region": dims.get("region").and_then(Value::as_str).unwrap_or(""),
            "preferred_value": json_i64(config.get("preferredValue")).unwrap_or(0),
            "granted_value": granted,
            "state": state,
            "state_detail": config.get("stateDetail").and_then(Value::as_str).unwrap_or(""),
            "create_time": pref.get("createTime").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    Ok(out)
}

/// Fan out CreateQuotaPreference for every gpu_family the live
/// cloudquotas API reports under compute.googleapis.com, in every region
/// passed (Python `gcp_request_all_families`). Uses the unified
/// GPUS-PER-GPU-FAMILY-per-project-region quota (the one that takes a
/// gpu_family dimension); the set of families is discovered, not
/// hardcoded — anything Google drops or adds tomorrow is picked up on the
/// next call without a package release.
pub async fn gcp_request_all_families(
    client: &CloudQuotasClient,
    new_limit: i64,
    regions: &[String],
    contact_email: &str,
    justification: &str,
) -> Result<Vec<Value>, CatalogError> {
    // Discover (a) the set of gpu_family values Google models for this
    // project, and (b) the UNION of every region any family is available
    // in. Per-family applicable_regions is conservative (it only lists
    // regions where the project has a non-default quota); the union gives
    // us the full lattice of regions Google serves any GPU SKU in.
    // Default behavior submits each family in every region in that union
    // — over-coverage; per-target "family not available in this region"
    // failures are captured as result-list entries, not exceptions.
    let mut families: std::collections::BTreeSet<String> = Default::default();
    let mut all_regions: std::collections::BTreeSet<String> = Default::default();
    for row in gcp_catalog(client).await? {
        if row.get("quota_id").and_then(Value::as_str)
            != Some("GPUS-PER-GPU-FAMILY-per-project-region")
        {
            continue;
        }
        let fam = row.get("gpu_family").and_then(Value::as_str).unwrap_or("");
        let region = row.get("region").and_then(Value::as_str).unwrap_or("");
        if !fam.is_empty() {
            families.insert(fam.to_string());
        }
        if !region.is_empty() {
            all_regions.insert(region.to_string());
        }
    }
    let requested: std::collections::BTreeSet<&str> = regions.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for fam in &families {
        let targets: Vec<&String> = if requested.is_empty() {
            all_regions.iter().collect()
        } else {
            all_regions
                .iter()
                .filter(|r| requested.contains(r.as_str()))
                .collect()
        };
        for region in targets {
            match super::quota_request::gcp_request_for_family(
                client,
                region,
                fam,
                new_limit,
                justification,
                contact_email,
            )
            .await
            {
                Ok(r) => {
                    let mut row = json!({
                        "provider": "gcp", "region": region,
                        "gpu_family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    out.push(row);
                }
                Err(err) => out.push(json!({
                    "provider": "gcp", "region": region,
                    "gpu_family": fam, "ok": false,
                    "error": format!("GoogleAPICallError: {err}"),
                })),
            }
        }
    }
    Ok(out)
}

/// Fan out Microsoft.Quota create_or_update for every distinct GPU family
/// the subscription advertises × every location the subscription serves
/// any GPU SKU in (Python `azure_request_all_families`). Same
/// default-union-of-all-locations pattern as GCP: per-family location
/// lists in az vm list-skus are conservative (only locations the
/// subscription has access to for that exact family), but request-all
/// defaults to the global union so the subscription builds quota
/// everywhere any family is available. Per-target "family not in this
/// location" failures are captured in the result list, not raised.
pub async fn azure_request_all_families(new_limit: i64, locations: &[String]) -> Vec<Value> {
    let catalog = azure_catalog().await;
    let mut families: std::collections::BTreeSet<String> = Default::default();
    let mut all_locs: std::collections::BTreeSet<String> = Default::default();
    for row in &catalog {
        let fam = row.get("family").and_then(Value::as_str).unwrap_or("");
        let loc = row.get("location").and_then(Value::as_str).unwrap_or("");
        if !fam.is_empty() {
            families.insert(fam.to_string());
        }
        if !loc.is_empty() {
            all_locs.insert(loc.to_string());
        }
    }
    let requested: std::collections::BTreeSet<&str> =
        locations.iter().map(String::as_str).collect();
    let target_locs: Vec<&String> = if requested.is_empty() {
        all_locs.iter().collect()
    } else {
        all_locs
            .iter()
            .filter(|l| requested.contains(l.as_str()))
            .collect()
    };
    let subscription = crate::config::azure_subscription_id();
    let mut out = Vec::new();
    for loc in target_locs {
        for fam in &families {
            let row = match super::quota_request::azure_request_increase(
                subscription,
                loc,
                fam,
                new_limit,
            )
            .await
            {
                Ok(r) if r.get("available").and_then(Value::as_bool) == Some(false) => json!({
                    "provider": "azure", "location": loc, "family": fam, "ok": false,
                    "error": r.get("reason").and_then(Value::as_str).unwrap_or("not available"),
                }),
                Ok(r) => {
                    let mut row = json!({
                        "provider": "azure", "location": loc, "family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    row
                }
                Err(err) => json!({
                    "provider": "azure", "location": loc, "family": fam, "ok": false,
                    "error": format!("AzureError: {err}"),
                }),
            };
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http};

    fn quota_infos_page(next: Option<&str>) -> String {
        let mut page = json!({
            "quotaInfos": [
                {
                    "name": "projects/p/locations/global/services/compute.googleapis.com/quotaInfos/NVIDIA-T4-GPUS-per-project-region",
                    "quotaId": "NVIDIA-T4-GPUS-per-project-region",
                    "metric": "compute.googleapis.com/nvidia_t4_gpus",
                    "metricDisplayName": "NVIDIA T4 GPUs",
                    "dimensionsInfos": [
                        {"dimensions": {}, "applicableLocations": ["us-central1", "us-east1"],
                         "details": {"value": "8"}},
                        {"dimensions": {}, "applicableLocations": [],
                         "details": {"value": "1"}},
                    ],
                },
                {
                    "name": ".../GPUS-PER-GPU-FAMILY-per-project-region",
                    "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                    "metric": "compute.googleapis.com/gpus_per_gpu_family",
                    "metricDisplayName": "",
                    "dimensionsInfos": [
                        {"dimensions": {"gpu_family": "NVIDIA_T4"}, "applicableLocations": ["us-central1"],
                         "details": {"value": "8"}},
                        {"dimensions": {"gpu_family": "NVIDIA_A100_80GB"}, "applicableLocations": ["europe-west4"],
                         "details": {}},
                    ],
                },
                {
                    "name": ".../CPUS-per-project-region",
                    "quotaId": "CPUS-per-project-region",
                    "metric": "compute.googleapis.com/cpus",
                    "dimensionsInfos": [
                        {"dimensions": {}, "applicableLocations": ["us-central1"],
                         "details": {"value": "2400"}},
                    ],
                },
            ],
        });
        if let Some(token) = next {
            page["nextPageToken"] = json!(token);
        }
        page.to_string()
    }

    #[tokio::test]
    async fn gcp_catalog_filters_gpu_quotas_and_flattens_dimensions() {
        let server = mock_http(vec![
            http_response(200, "OK", &quota_infos_page(Some("page-2"))),
            http_response(200, "OK", &quota_infos_page(None)),
        ])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "test-project");
        let rows = gcp_catalog(&client).await.unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with(
                "GET /projects/test-project/locations/global/services/compute.googleapis.com/quotaInfos "
            ),
            "{}",
            requests[0]
        );
        assert!(requests[1].contains("pageToken=page-2"), "{}", requests[1]);
        server.stop();

        // Two identical pages concatenate (Python paginates the same way).
        assert_eq!(rows.len(), 2 * 5, "{rows:?}");
        let first = &rows[0];
        assert_eq!(
            first["quota_id"],
            json!("NVIDIA-T4-GPUS-per-project-region")
        );
        assert_eq!(first["metric"], json!("NVIDIA T4 GPUs"));
        assert_eq!(first["region"], json!("us-central1"));
        assert_eq!(first["limit"], json!(8));
        // Empty applicableLocations -> one "global" row.
        assert!(rows
            .iter()
            .any(|r| r["region"] == json!("global") && r["limit"] == json!(1)));
        // Empty metricDisplayName falls back to the metric path.
        let family_row = rows
            .iter()
            .find(|r| r["gpu_family"] == json!("NVIDIA_T4"))
            .expect("gpu_family row");
        assert_eq!(
            family_row["metric"],
            json!("compute.googleapis.com/gpus_per_gpu_family")
        );
        // Missing details.value -> null limit (Python None).
        let no_value = rows
            .iter()
            .find(|r| r["gpu_family"] == json!("NVIDIA_A100_80GB"))
            .expect("A100 row");
        assert_eq!(no_value["limit"], json!(null));
        assert_eq!(no_value["region"], json!("europe-west4"));
        // CPUS is not GPU-related and was filtered out.
        assert!(!rows
            .iter()
            .any(|r| r["quota_id"] == json!("CPUS-per-project-region")));
    }

    #[tokio::test]
    async fn gcp_catalog_http_error_is_an_api_error() {
        let server = mock_http(vec![http_response(
            403,
            "Forbidden",
            r#"{"error": {"message": "denied"}}"#,
        )])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "test-project");
        let err = gcp_catalog(&client).await.unwrap_err();
        assert!(err.to_string().contains("HTTP 403"), "{err}");
        server.stop();
    }

    #[test]
    fn azure_rows_from_skus_filters_families_and_expands_locations() {
        let skus = serde_json::from_str::<Vec<Value>>(
            r#"[
                {"name": "Standard_NC4as_T4_v3", "family": "Standard NCASv3_T4 Family",
                 "locations": ["eastus", "westus3"]},
                {"name": "Standard_D4s_v5", "family": "standardDSv5Family",
                 "locations": ["eastus"]},
                {"name": "Standard_NC24ads_A100_v4", "family": "StandardNCADSA100v4Family",
                 "locations": []}
            ]"#,
        )
        .unwrap();
        let rows = azure_rows_from_skus(&skus);
        // standardDSv5Family has no NC/ND/NV/GPU token; the A100 row has
        // no locations.
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["family"], json!("Standard NCASv3_T4 Family"));
        assert_eq!(rows[0]["sku"], json!("Standard_NC4as_T4_v3"));
        assert_eq!(rows[0]["location"], json!("eastus"));
        assert_eq!(rows[1]["location"], json!("westus3"));
    }

    #[tokio::test]
    async fn provider_catalog_unknown_provider_is_an_error_row() {
        let rows = provider_catalog("dcloud", None).await.unwrap();
        assert_eq!(
            rows,
            vec![json!({
                "provider": "dcloud",
                "ok": false,
                "error": "no catalog impl for this provider",
            })]
        );
    }

    #[tokio::test]
    async fn gcp_request_status_buckets_states_and_parses_int64_strings() {
        let page = json!({
            "quotaPreferences": [
                {
                    "name": "projects/p/locations/global/quotaPreferences/a",
                    "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                    "dimensions": {"gpu_family": "NVIDIA_T4", "region": "us-central1"},
                    "quotaConfig": {"preferredValue": "16", "grantedValue": "8",
                                    "stateDetail": "Approved by reviewer"},
                    "createTime": "2026-06-01T00:00:00Z",
                },
                {
                    "name": ".../b",
                    "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                    "dimensions": {"gpu_family": "NVIDIA_L4", "region": "us-east1"},
                    "quotaConfig": {"preferredValue": "4", "stateDetail": "Partially approved"},
                    "reconciling": true,
                    "createTime": "2026-06-02T00:00:00Z",
                },
                {
                    "name": ".../c",
                    "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                    "dimensions": {},
                    "quotaConfig": {"preferredValue": "2", "stateDetail": "Quota increase denied"},
                },
            ],
        });
        let server = mock_http(vec![http_response(200, "OK", &page.to_string())]).await;
        let client = CloudQuotasClient::for_test(&server.base_url, "p");
        let rows = gcp_request_status(&client).await.unwrap();
        server.stop();
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0]["state"], json!("approved"));
        assert_eq!(rows[0]["preferred_value"], json!(16));
        assert_eq!(rows[0]["granted_value"], json!(8));
        assert_eq!(rows[0]["create_time"], json!("2026-06-01T00:00:00Z"));
        // reconciling wins over the stateDetail bucket; granted 0/missing -> null.
        assert_eq!(rows[1]["state"], json!("reconciling"));
        assert_eq!(rows[1]["granted_value"], json!(null));
        assert_eq!(rows[2]["state"], json!("denied"));
        assert_eq!(rows[2]["gpu_family"], json!(""));
    }

    #[tokio::test]
    async fn gcp_request_all_families_discovers_families_and_region_union() {
        let infos = json!({
            "quotaInfos": [
                {
                    "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                    "metric": "compute.googleapis.com/gpus_per_gpu_family",
                    "dimensionsInfos": [
                        {"dimensions": {"gpu_family": "NVIDIA_L4"}, "applicableLocations": ["us-east1"],
                         "details": {"value": "8"}},
                        {"dimensions": {"gpu_family": "NVIDIA_T4"}, "applicableLocations": ["us-central1"],
                         "details": {"value": "8"}},
                    ],
                },
                {
                    "quotaId": "NVIDIA-T4-GPUS-per-project-region",
                    "metric": "compute.googleapis.com/nvidia_t4_gpus",
                    "dimensionsInfos": [
                        {"dimensions": {}, "applicableLocations": ["us-west1"],
                         "details": {"value": "8"}},
                    ],
                },
            ],
        });
        let pref = |name: &str| http_response(200, "OK", &json!({"name": name}).to_string());
        let server = mock_http(vec![
            http_response(200, "OK", &infos.to_string()),
            // L4: us-central1, us-east1 (sorted union; legacy-only us-west1 excluded).
            pref(".../l4-uc1"),
            pref(".../l4-ue1"),
            // T4: us-central1, us-east1.
            pref(".../t4-uc1"),
            pref(".../t4-ue1"),
        ])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "p");
        let rows = gcp_request_all_families(&client, 16, &[], "e@x", "j")
            .await
            .unwrap();
        server.stop();
        assert_eq!(rows.len(), 4, "{rows:?}");
        // families sorted: NVIDIA_L4 first; regions sorted within a family.
        let keys: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| {
                (
                    r["gpu_family"].as_str().unwrap(),
                    r["region"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("NVIDIA_L4", "us-central1"),
                ("NVIDIA_L4", "us-east1"),
                ("NVIDIA_T4", "us-central1"),
                ("NVIDIA_T4", "us-east1"),
            ]
        );
        assert!(rows.iter().all(|r| r["ok"] == json!(true)));
        // Region filter intersects against the union.
        let server = mock_http(vec![
            http_response(200, "OK", &infos.to_string()),
            pref(".../l4-ue1"),
            pref(".../t4-ue1"),
        ])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "p");
        let rows = gcp_request_all_families(&client, 16, &["us-east1".to_string()], "e@x", "j")
            .await
            .unwrap();
        server.stop();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows.iter().all(|r| r["region"] == json!("us-east1")));
    }
}

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
    let env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
    )
    .expect("GCP project binding is missing from the capability catalog");
    std::env::var(env).unwrap_or_else(|_| "wisent-480400".to_string())
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
                    "provider": crate::capabilities::ProviderId::Gcp.as_str(),
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
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
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
            "provider": crate::capabilities::ProviderId::Azure.as_str(),
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
                "provider": crate::capabilities::ProviderId::Azure.as_str(),
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
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
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
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
                    "ok": false,
                    "error": err.to_string(),
                })];
            }
        };
        if !status.is_success() {
            return vec![json!({
                "provider": crate::capabilities::ProviderId::Azure.as_str(),
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
    let adapter = crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, provider)
        .map(|variant| variant.adapter);
    match adapter {
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Gcp,
        )) => {
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
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Azure,
        )) => Ok(azure_catalog().await),
        _ => Ok(vec![json!({
            "provider": provider,
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
                        "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region,
                        "gpu_family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    out.push(row);
                }
                Err(err) => out.push(json!({
                    "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region,
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
                    "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": false,
                    "error": r.get("reason").and_then(Value::as_str).unwrap_or("not available"),
                }),
                Ok(r) => {
                    let mut row = json!({
                        "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    row
                }
                Err(err) => json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": false,
                    "error": format!("AzureError: {err}"),
                }),
            };
            out.push(row);
        }
    }
    out
}

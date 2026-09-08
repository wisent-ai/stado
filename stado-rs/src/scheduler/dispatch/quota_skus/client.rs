//! Bearer-authenticated Cloud Quotas REST v1 client, its endpoint
//! constants, and the error type every catalog call raises.

use std::sync::Arc;

use serde_json::Value;

/// Cloud Quotas API v1 base.
pub const CLOUD_QUOTAS_BASE: &str = "https://cloudquotas.googleapis.com/v1";
/// OAuth scope for the Cloud Quotas REST read.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

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

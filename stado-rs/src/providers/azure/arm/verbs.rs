//! The REST verbs layered on [`ArmClient`]'s transport: typed GET/POST/PUT
//! /DELETE with ARM's 404 and long-running-operation conventions, the VM
//! and quota readers, and the VM resource-path builder.

use serde_json::{json, Value};

use super::{ArmClient, AzureError, COMPUTE_API_VERSION};

impl ArmClient {
    /// GET a JSON resource; non-2xx is an [`AzureError::Api`]. `url` may
    /// be a path or an absolute URL (LRO poll).
    pub async fn get(&self, url: &str, desc: &str) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::GET, url, None).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// POST one JSON request and decode the JSON response. Resource Graph,
    /// Cost Management, and Monitor use POST query endpoints even for
    /// read-only operations; exposing the typed transport keeps their
    /// authentication path identical to VM lifecycle calls.
    pub async fn post_json(
        &self,
        path: &str,
        body: &Value,
        desc: &str,
    ) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::POST, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// POST a lifecycle action whose successful ARM response may have an
    /// empty body (for example VM start/deallocate).
    pub async fn post_action(&self, path: &str, desc: &str) -> Result<(), AzureError> {
        let body = json!({});
        let response = self.send(reqwest::Method::POST, path, Some(&body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(())
    }

    /// GET that maps 404 to `None` (Python's `except ResourceNotFoundError`).
    pub async fn get_allow_404(&self, path: &str, desc: &str) -> Result<Option<Value>, AzureError> {
        let response = self.send(reqwest::Method::GET, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// DELETE a resource; `false` on 404 (idempotent NotFound). Does NOT
    /// wait for the returned operation — Python's `begin_delete` call
    /// never invokes `op.result()` either.
    pub async fn delete_allow_404(&self, path: &str, desc: &str) -> Result<bool, AzureError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(true)
    }

    /// DELETE a resource and wait for Azure's operation to finish. Protected
    /// extension deletion uses this stronger form so decrypted handler
    /// settings are removed before dispatch is reported successful.
    pub(in crate::providers::azure) async fn delete_lro_allow_404(
        &self,
        path: &str,
        desc: &str,
    ) -> Result<bool, AzureError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let async_op = header("azure-asyncoperation");
        let location = header("location");
        if let Some(url) = async_op {
            self.poll_async_operation(&url, desc).await?;
        } else if let Some(url) = location {
            self.poll_location(&url, desc).await?;
        }
        Ok(true)
    }

    /// PUT a resource body and wait for the async operation to reach a
    /// terminal state (Python SDK `op.result()`). Returns the parsed PUT
    /// response body.
    pub async fn put_lro(&self, path: &str, body: &Value, desc: &str) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::PUT, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let async_op = header("azure-asyncoperation");
        let location = header("location");
        let text = response.text().await.unwrap_or_default();
        if let Some(url) = async_op {
            self.poll_async_operation(&url, desc).await?;
        } else if let Some(url) = location {
            self.poll_location(&url, desc).await?;
        }
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// GET a VM with `$expand=instanceView`; None on 404.
    pub async fn get_vm(&self, rg: &str, name: &str) -> Result<Option<Value>, AzureError> {
        let path = format!(
            "{}?$expand=instanceView&api-version={COMPUTE_API_VERSION}",
            vm_path(self.subscription(), rg, name)
        );
        self.get_allow_404(&path, &format!("get VM {name}")).await
    }

    /// List VMs in the resource group (nextLink-paginated), as raw JSON.
    pub async fn list_vms(&self, rg: &str) -> Result<Vec<Value>, AzureError> {
        let mut out = Vec::new();
        let mut url = format!(
            "/subscriptions/{}/resourceGroups/{rg}\
             /providers/Microsoft.Compute/virtualMachines?api-version={COMPUTE_API_VERSION}",
            self.subscription()
        );
        loop {
            let page = self.get(&url, "list virtualMachines").await?;
            if let Some(vms) = page.get("value").and_then(Value::as_array) {
                out.extend(vms.iter().cloned());
            }
            match page.get("nextLink").and_then(Value::as_str) {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        Ok(out)
    }
    /// List regional Microsoft.Compute quota usages.
    pub async fn list_usages(&self, location: &str) -> Result<Vec<Value>, AzureError> {
        let path = format!(
            "/subscriptions/{}/providers/Microsoft.Compute/locations/{location}\
             /usages?api-version={COMPUTE_API_VERSION}",
            self.subscription()
        );
        let page = self
            .get(&path, &format!("list compute usages in {location}"))
            .await?;
        Ok(page
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

/// ARM resource path of a VM (no api-version). Crate-visible for the
/// agent's self-delete ([`crate::providers::local::cloud::azure_self`]).
pub(crate) fn vm_path(subscription: &str, rg: &str, name: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{rg}\
         /providers/Microsoft.Compute/virtualMachines/{name}"
    )
}

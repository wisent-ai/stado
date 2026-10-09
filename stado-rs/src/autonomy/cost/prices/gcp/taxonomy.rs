//! The Pricing API's product taxonomy of Compute Engine SKUs: which machine
//! family or GPU model each SKU prices.

use std::collections::HashMap;

use serde_json::Value;

use super::COMPUTE_ENGINE_SERVICE;

/// What a Compute Engine SKU prices, from the Pricing API's product taxonomy
/// (`GCP > Compute > GCE > VMs On Demand > Cores: Per Core > N1`,
/// `GCP > Compute > GPUs > GPUs On Demand > A100`). The v1 catalog that
/// carries prices names the family and the GPU model only in its sentence.
pub(super) enum SkuTaxonomy {
    /// A per-core or per-GiB price of one machine family, lowercased (`n1`).
    Family(String),
    /// A per-accelerator price, as Stado's canonical accelerator id.
    Accelerator(&'static str),
}

pub(super) async fn gcp_sku_taxonomy(
    client: &reqwest::Client,
    token: &str,
) -> Result<HashMap<String, SkuTaxonomy>, String> {
    let filter = format!("service=\"services/{COMPUTE_ENGINE_SERVICE}\"");
    let encode =
        |value: &str| url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    let mut taxonomy = HashMap::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut url = format!(
            "https://cloudbilling.googleapis.com/v2beta/skus?pageSize=5000&filter={}",
            encode(&filter)
        );
        if let Some(page) = page_token.as_deref() {
            url.push_str("&pageToken=");
            url.push_str(&encode(page));
        }
        let response = crate::wait::request(client.get(&url).bearer_auth(token))
            .await
            .map_err(|error| format!("Pricing API SKU taxonomy read: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Pricing API SKU taxonomy HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }
        let document: Value = response
            .json()
            .await
            .map_err(|error| format!("Pricing API SKU taxonomy read: {error}"))?;
        for sku in document
            .get("skus")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(sku_id), Some(classified)) =
                (sku.get("skuId").and_then(Value::as_str), sku_taxonomy(sku))
            {
                taxonomy.insert(sku_id.to_string(), classified);
            }
        }
        page_token = document
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|page| !page.is_empty())
            .map(str::to_string);
        if page_token.is_none() {
            return Ok(taxonomy);
        }
    }
}

fn sku_taxonomy(sku: &Value) -> Option<SkuTaxonomy> {
    let categories: Vec<&str> = sku
        .pointer("/productTaxonomy/taxonomyCategories")?
        .as_array()?
        .iter()
        .filter_map(|category| category.get("category").and_then(Value::as_str))
        .collect();
    match categories.as_slice() {
        ["GCP", "Compute", "GCE", _, "Cores: Per Core" | "Memory: Per GB", family] => {
            Some(SkuTaxonomy::Family(family.to_ascii_lowercase()))
        }
        ["GCP", "Compute", "GPUs", _, model] => {
            gcp_accelerator(model).map(SkuTaxonomy::Accelerator)
        }
        _ => None,
    }
}

/// The taxonomy's GPU model as Stado's canonical accelerator id; a model
/// Stado has no id for is not priced.
fn gcp_accelerator(model: &str) -> Option<&'static str> {
    Some(match model {
        "T4" => "nvidia-tesla-t4",
        "P100" => "nvidia-tesla-p100",
        "V100" => "nvidia-tesla-v100",
        "A100" => "nvidia-tesla-a100",
        "A10080GB" => "nvidia-a100-80gb",
        "L4" => "nvidia-l4",
        "H100" => "nvidia-h100-80gb",
        "H200" => "nvidia-h200-141gb",
        "A4B200" => "nvidia-b200-180gb",
        "RTX6000" => "nvidia-rtx-pro-6000",
        _ => return None,
    })
}

//! Instance-scoped GCE reads: single-instance status and the paginated
//! aggregated list. Both are consumed by the provider component, so they
//! carry the narrowest visibility that reaches it.

use serde_json::Value;

use super::{GceClient, GceError};

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl GceClient {
    /// `GET .../zones/{zone}/instances/{name}` status, or None when the
    /// instance does not exist.
    pub(in crate::providers::gcp) async fn instance_status(
        &self,
        zone: &str,
        name: &str,
    ) -> Result<Option<String>, GceError> {
        let path = format!("/projects/{}/zones/{zone}/instances/{name}", self.project());
        let Some(instance) = self
            .get_allow_404(&path, &format!("get instance {name}@{zone}"))
            .await?
        else {
            return Ok(None);
        };
        if !instance.is_object() {
            return Err(GceError::Api(format!(
                "GCE get instance {name}@{zone} at {path} returned {}, not an instance object",
                json_kind(&instance)
            )));
        }
        let status = instance.get("status").and_then(Value::as_str).ok_or_else(|| {
            let observed = match instance.get("status") {
                Some(value) => json_kind(value),
                None => "missing",
            };
            GceError::Api(format!(
                "GCE get instance {name}@{zone} at {path} returned status as {observed}, not a lifecycle state"
            ))
        })?;
        Ok(Some(status.to_owned()))
    }

    /// `GET .../aggregated/instances?filter=...`, flattened to
    /// `(zone, instance-json)` pairs across all pages.
    pub(in crate::providers::gcp) async fn aggregated_instances(
        &self,
        filter: &str,
    ) -> Result<Vec<(String, Value)>, GceError> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut path = format!(
                "/projects/{}/aggregated/instances?filter={}",
                self.project(),
                crate::queue::gcs::percent_encode(filter)
            );
            if let Some(token) = &page_token {
                path.push_str(&format!(
                    "&pageToken={}",
                    crate::queue::gcs::percent_encode(token)
                ));
            }
            let response = self.get(&path, "aggregatedList instances").await?;
            let Value::Object(mut page) = response else {
                return Err(GceError::Api(format!(
                    "GCE aggregatedList instances at {path} returned no response object"
                )));
            };
            if let Some(unreachables) = page.remove("unreachables") {
                match unreachables {
                    Value::Array(zones) if zones.is_empty() => {}
                    Value::Array(zones) => {
                        return Err(GceError::Api(format!(
                            "GCE aggregatedList instances at {path} could not reach scopes: {zones:?}"
                        )));
                    }
                    value => {
                        return Err(GceError::Api(format!(
                            "GCE aggregatedList instances at {path} returned unreachables as {}, not an array",
                            json_kind(&value)
                        )));
                    }
                }
            }
            let next = match page.remove("nextPageToken") {
                None => None,
                Some(Value::String(token)) if !token.is_empty() => Some(token),
                Some(value) => {
                    return Err(GceError::Api(format!(
                        "GCE aggregatedList instances at {path} returned nextPageToken as an empty or malformed {}",
                        json_kind(&value)
                    )));
                }
            };
            match page.remove("items") {
                Some(Value::Object(items)) => {
                    for (scope, scoped) in items {
                        let Value::Object(mut fields) = scoped else {
                            return Err(GceError::Api(format!(
                                "GCE aggregatedList instances at {path} returned malformed scope {scope}"
                            )));
                        };
                        match fields.remove("instances") {
                            Some(Value::Array(instances)) => {
                                let zone = scope
                                    .strip_prefix("zones/")
                                    .filter(|zone| !zone.is_empty() && !zone.contains('/'))
                                    .ok_or_else(|| {
                                        GceError::Api(format!(
                                            "GCE aggregatedList instances at {path} returned an invalid zone scope {scope}"
                                        ))
                                    })?;
                                for instance in instances {
                                    out.push((zone.to_owned(), instance));
                                }
                            }
                            None if fields
                                .get("warning")
                                .and_then(|warning| warning.get("code"))
                                .and_then(Value::as_str)
                                == Some("NO_RESULTS_ON_PAGE") => {}
                            other => {
                                let observed = match other.as_ref() {
                                    Some(value) => json_kind(value),
                                    None => "missing",
                                };
                                let warning = fields
                                    .get("warning")
                                    .and_then(|value| value.get("code"))
                                    .and_then(Value::as_str);
                                return Err(GceError::Api(format!(
                                    "GCE aggregatedList instances at {path} cannot read instances in {scope}: {observed}; warning code={warning:?}"
                                )));
                            }
                        }
                    }
                }
                None if page
                    .get("warning")
                    .and_then(|warning| warning.get("code"))
                    .and_then(Value::as_str)
                    == Some("NO_RESULTS_ON_PAGE") => {}
                other => {
                    let observed = match other.as_ref() {
                        Some(value) => json_kind(value),
                        None => "missing",
                    };
                    let warning = page
                        .get("warning")
                        .and_then(|value| value.get("code"))
                        .and_then(Value::as_str);
                    return Err(GceError::Api(format!(
                        "GCE aggregatedList instances at {path} cannot read items: {observed}; warning code={warning:?}"
                    )));
                }
            }
            match next {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }
        Ok(out)
    }
}

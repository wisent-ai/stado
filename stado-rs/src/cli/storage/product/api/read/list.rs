//! One namespace listing.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn list(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<Vec<Value>, CmdError> {
        let endpoint = self.endpoint(
            "/api/object/list",
            &[("namespace", namespace), ("prefix", prefix)],
        )?;
        let bearer = self.release_bearer_for(namespace, prefix).await?;
        let response = self
            .request_as(reqwest::Method::GET, endpoint, bearer.as_deref())
            .send()
            .await?;
        let payload: RemoteObjectListResponse = self
            .response_json(response, "object list", bearer.as_deref())
            .await?;
        let mut values = Vec::with_capacity(payload.objects.len());
        for item in payload.objects {
            let object = crate::remote::object_store::ObjectRef::parse(&item.uri).map_err(|error| {
                CmdError::click(format!(
                    "Stado object API returned an invalid object-list URI: {error}"
                ))
            })?;
            // Two different faults, and they were one refusal. An item whose
            // `uri`, `namespace` and `key` disagree is a broken store and
            // must stop the read. An item outside the requested prefix is a
            // gateway that answered a wider question than it was asked —
            // `prefix=queue/` returning `queue_priority/` — and the honest
            // response is to keep what was asked for, because a fleet still
            // running that gateway must not make this reader refuse a store
            // that holds exactly the right objects.
            if object.namespace() != namespace
                || item.namespace.as_str() != object.namespace()
                || item.key.as_str() != object.key()
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object-list item",
                ));
            }
            if !object.key().starts_with(prefix) {
                continue;
            }
            values.push(json!({
                "uri": item.uri,
                "namespace": item.namespace,
                "key": item.key,
                "size": item.size,
                "updated_at": item.updated_at,
                "metadata": item.metadata,
            }));
        }
        Ok(values)
    }
}

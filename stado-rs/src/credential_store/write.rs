//! CRUD for the selected credential store and explicit-backend operations used
//! by migration. Skarbiec calls use a direct client to avoid routing recursion;
//! the file backend keeps values plus item types in one owner-only JSON file.

use serde_json::{Map, Value};

use super::file::TYPE_METADATA;
use super::{selected, Backend};
use crate::skarbiec::{Client, ItemInfo, SkarbiecError};

fn type_map(document: &Value) -> Option<&Map<String, Value>> {
    document.get(TYPE_METADATA).and_then(Value::as_object)
}

fn set_type(document: &mut Value, id: &str, item_type: &str) {
    if document.get(TYPE_METADATA).and_then(Value::as_object).is_none() {
        document[TYPE_METADATA] = Value::Object(Map::new());
    }
    document[TYPE_METADATA][id] = Value::String(item_type.to_string());
}

fn direct_client(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<Client, SkarbiecError> {
    let Backend::Skarbiec { url: store_url } = backend else {
        return Err(SkarbiecError::Deployment(
            "internal error: direct Skarbiec client requested for file backend".to_string(),
        ));
    };
    Client::direct(
        store_url.as_deref().unwrap_or(url),
        consumer,
        token_file,
    )
}

pub(crate) async fn read_item_at(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
) -> Result<Value, SkarbiecError> {
    match backend {
        Backend::Skarbiec { .. } => direct_client(backend, url, consumer, token_file)?
            .read_item(id)
            .await,
        Backend::File { path } => super::file::file_read_item(path, id),
    }
}

pub(crate) async fn write_item_at(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
    item_type: &str,
    value: &Value,
) -> Result<(), SkarbiecError> {
    match backend {
        Backend::Skarbiec { .. } => direct_client(backend, url, consumer, token_file)?
            .write_item(id, item_type, value)
            .await,
        Backend::File { path } => {
            if id == TYPE_METADATA {
                return Err(SkarbiecError::Deployment(format!(
                    "credential item id {TYPE_METADATA:?} is reserved"
                )));
            }
            let mut document = super::file::file_load(path)?;
            document[id] = value.clone();
            set_type(&mut document, id, item_type);
            super::file::file_store(path, &document)
        }
    }
}

pub(crate) async fn delete_item_at(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
) -> Result<(), SkarbiecError> {
    match backend {
        Backend::Skarbiec { .. } => direct_client(backend, url, consumer, token_file)?
            .delete_item(id)
            .await,
        Backend::File { path } => {
            let mut document = super::file::file_load(path)?;
            if let Some(items) = document.as_object_mut() {
                items.remove(id);
            }
            if let Some(types) = document
                .get_mut(TYPE_METADATA)
                .and_then(Value::as_object_mut)
            {
                types.remove(id);
            }
            super::file::file_store(path, &document)
        }
    }
}

pub(crate) async fn list_items_at(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<Vec<ItemInfo>, SkarbiecError> {
    match backend {
        Backend::Skarbiec { .. } => direct_client(backend, url, consumer, token_file)?
            .list_items()
            .await,
        Backend::File { path } => {
            let document = super::file::file_load(path)?;
            let types = type_map(&document);
            let mut items = document
                .as_object()
                .into_iter()
                .flat_map(|values| values.keys())
                .filter(|id| id.as_str() != TYPE_METADATA)
                .map(|id| ItemInfo {
                    id: id.clone(),
                    item_type: types
                        .and_then(|values| values.get(id))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    tags: None,
                    updated_at: None,
                    deleted: None,
                    versions: None,
                })
                .collect::<Vec<_>>();
            items.sort_by(|left, right| left.id.cmp(&right.id));
            Ok(items)
        }
    }
}

pub async fn write_item_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
    item_type: &str,
    value: &Value,
) -> Result<(), SkarbiecError> {
    write_item_at(
        &selected()?,
        url,
        consumer,
        token_file,
        id,
        item_type,
        value,
    )
    .await
}

pub async fn delete_item_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
) -> Result<(), SkarbiecError> {
    delete_item_at(&selected()?, url, consumer, token_file, id).await
}

pub async fn list_items_with(
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<Vec<ItemInfo>, SkarbiecError> {
    list_items_at(&selected()?, url, consumer, token_file).await
}

pub async fn list_ids_with(
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<Vec<String>, SkarbiecError> {
    Ok(list_items_with(url, consumer, token_file)
        .await?
        .into_iter()
        .map(|item| item.id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]

    async fn file_backend_roundtrip() {
        let dir = std::env::temp_dir().join(format!("stado-cred-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("store.json");
        std::env::set_var("STADO_CREDENTIAL_STORE", format!("file://{}", path.display()));
        write_item_with("unused", "unused", "unused", "alpha", "token", &serde_json::json!({"token": "v"}))
            .await
            .expect("write");
        let ids = list_ids_with("unused", "unused", "unused").await.expect("list");
        assert_eq!(ids, vec!["alpha".to_string()]);
        delete_item_with("unused", "unused", "unused", "alpha")
            .await
            .expect("delete");
        let ids = list_ids_with("unused", "unused", "unused").await.expect("list");
        assert!(ids.is_empty());
        std::env::remove_var("STADO_CREDENTIAL_STORE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

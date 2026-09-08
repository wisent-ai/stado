//! Publishing one validated, authorized composition: assemble the staged
//! chunks, verify the digest, land the object with its metadata, and clean the
//! staging prefix up.

use std::collections::BTreeMap;
use std::io::Write;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::object_store::ObjectRef;

use crate::dashboard::listener::auth::release_upload_target_key;
use crate::dashboard::listener::{http_status, storage_error_response, Dashboard, Response};

use super::{object_compose_error, object_compose_response, ObjectComposeRequest};

impl Dashboard {
    pub(crate) async fn publish_composition(
        &self,
        payload: &ObjectComposeRequest,
        object: &ObjectRef,
        chunks: &[(ObjectRef, [u8; 32])],
        metadata: &BTreeMap<String, String>,
        expected_upload_digest: [u8; 32],
    ) -> Response {
        // Hold across publication, metadata, mirror writes and chunk cleanup:
        // a handoff must not capture half of an accepted composition.
        let _write_guard = match self.storage_write_guard() {
            Ok(guard) => guard,
            Err(error) => return storage_error_response(error),
        };
        let mut staged = match tempfile::NamedTempFile::new() {
            Ok(staged) => staged,
            Err(error) => return object_compose_error(http_status("500"), error.to_string()),
        };
        let mut object_digest = Sha256::new();
        let mut assembled_size = 0usize;
        for ((chunk_object, expected_digest), declared) in chunks.iter().zip(&payload.chunks) {
            let bytes = match self.store.read_bytes(&chunk_object.storage_path()).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => {
                    return object_compose_response(
                        http_status("404"),
                        json!({"state": "absent", "uri": chunk_object.to_string()}),
                    )
                }
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            let actual_digest: [u8; 32] = Sha256::digest(&bytes).into();
            if bytes.len() != declared.size || actual_digest != *expected_digest {
                return object_compose_error(
                    http_status("422"),
                    format!("stored chunk does not match {}", chunk_object),
                );
            }
            if let Err(error) = staged.write_all(&bytes) {
                return object_compose_error(http_status("500"), error.to_string());
            }
            object_digest.update(&bytes);
            assembled_size += bytes.len();
        }
        if assembled_size != payload.size {
            return object_compose_error(
                http_status("422"),
                "assembled object size differs from the composition request",
            );
        }
        let assembled_digest: [u8; 32] = object_digest.finalize().into();
        if assembled_digest != expected_upload_digest {
            return object_compose_error(
                http_status("422"),
                "assembled object SHA-256 differs from upload_id",
            );
        }
        if let Err(error) = staged.flush() {
            return object_compose_error(http_status("500"), error.to_string());
        }

        let target_path = object.storage_path();
        if payload.if_absent {
            let created = match self
                .store
                .upload_file_if_absent(&target_path, staged.path())
                .await
            {
                Ok(created) => created,
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            if !created {
                let existing = match self.store.read_bytes(&target_path).await {
                    Ok(Some(existing)) => existing,
                    Ok(None) => {
                        return object_compose_error(
                            http_status("500"),
                            "object disappeared after create-only conflict",
                        )
                    }
                    Err(error) => {
                        return object_compose_error(http_status("500"), error.to_string())
                    }
                };
                let existing_digest: [u8; 32] = Sha256::digest(&existing).into();
                if existing.len() != payload.size || existing_digest != expected_upload_digest {
                    return object_compose_response(
                        http_status("409"),
                        json!({
                            "error": "object exists with different content",
                            "uri": object.to_string(),
                        }),
                    );
                }
            }
        } else {
            let bytes = match std::fs::read(staged.path()) {
                Ok(bytes) => bytes,
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            if let Err(error) = self.store.upload_bytes(&target_path, &bytes).await {
                return object_compose_error(http_status("500"), error.to_string());
            }
        }

        if let Err(error) = self
            .store
            .backend()
            .set_metadata(&target_path, metadata)
            .await
        {
            return object_compose_error(http_status("500"), error.to_string());
        }
        let landed = match self
            .store
            .backend()
            .list_blobs_with_meta(&target_path)
            .await
        {
            Ok(landed) => landed,
            Err(error) => return object_compose_error(http_status("500"), error.to_string()),
        };
        let Some(blob) = landed.into_iter().find(|blob| blob.name == target_path) else {
            return object_compose_error(
                http_status("500"),
                format!("object metadata verification could not find {object}"),
            );
        };
        if metadata
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .any(|(key, value)| blob.metadata.get(key) != Some(value))
        {
            return object_compose_error(
                http_status("500"),
                format!("object metadata verification failed for {object}"),
            );
        }

        let cleanup_paths = if payload.if_absent {
            let prefix = format!("{target_path}.__stado_upload/");
            match self.store.list_paths(&prefix, usize::default()).await {
                Ok(paths) => paths
                    .into_iter()
                    .filter(|path| {
                        ObjectRef::from_storage_path(path).is_ok_and(|candidate| {
                            candidate.namespace() == object.namespace()
                                && release_upload_target_key(candidate.key()) == Some(object.key())
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            }
        } else {
            chunks
                .iter()
                .map(|(chunk, _)| chunk.storage_path())
                .collect::<Vec<_>>()
        };
        for chunk_path in cleanup_paths {
            if let Err(error) = self.store.delete_blob(&chunk_path).await {
                return object_compose_error(http_status("500"), error.to_string());
            }
        }

        object_compose_response(
            http_status("200"),
            json!({
                "state": "stored",
                "uri": object.to_string(),
                "content_type": payload.content_type,
            }),
        )
    }
}

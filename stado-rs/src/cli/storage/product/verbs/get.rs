//! `stado storage get` and `stado storage objects`.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageGetArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    /// Local destination file, or '-' for stdout.
    destination: String,
}

#[derive(Args, Debug)]
pub struct StorageObjectsArgs {
    /// Logical namespace, for example `images` or `checkpoints`.
    namespace: String,
    /// Optional key prefix inside the namespace.
    #[arg(default_value = "")]
    prefix: String,
    #[arg(long)]
    json: bool,
}

pub(in crate::cli::storage) async fn get(args: &StorageGetArgs) -> Result<(), CmdError> {
    let bytes = fetch_object(&args.uri).await?;
    if args.destination == "-" {
        let mut out = std::io::stdout().lock();
        out.write_all(&bytes)?;
        out.flush()?;
    } else {
        std::fs::write(&args.destination, bytes)?;
    }
    Ok(())
}

pub(in crate::cli::storage) async fn objects(args: &StorageObjectsArgs) -> Result<(), CmdError> {
    let storage_prefix =
        crate::object_store::ObjectRef::namespace_prefix(&args.namespace, &args.prefix)?;
    let values = if let Some(remote) =
        RemoteObjectApi::configured_for_list(&args.namespace, &args.prefix)?
    {
        remote.list(&args.namespace, &args.prefix).await?
    } else {
        let store = JobStorage::new().await?;
        let blobs = store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut values = Vec::with_capacity(blobs.len());
        for blob in blobs {
            let object = crate::object_store::ObjectRef::from_storage_path(&blob.name)?;
            values.push(json!({
                "uri": object.to_string(),
                "namespace": object.namespace(),
                "key": object.key(),
                "size": blob.size,
                "updated_at": render_optional_stamp(blob.updated),
                "metadata": blob.metadata,
            }));
        }
        values
    };
    if args.json {
        echo_json(&json!({"objects": values}))?;
    } else {
        let rows = values
            .iter()
            .map(|value| {
                vec![
                    value["uri"].as_str().unwrap_or_default().to_string(),
                    value
                        .get("size")
                        .and_then(Value::as_u64)
                        .map_or_else(|| "?".to_string(), |size| size.to_string()),
                    value["updated_at"].as_str().unwrap_or_default().to_string(),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["URI", "BYTES", "UPDATED"], &rows);
    }
    Ok(())
}

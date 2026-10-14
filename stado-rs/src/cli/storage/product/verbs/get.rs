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
        crate::remote::object_store::ObjectRef::namespace_prefix(&args.namespace, &args.prefix)?;
    let values = if let Some(remote) =
        RemoteObjectApi::configured_for_list(&args.namespace, &args.prefix)?
    {
        remote.list(&args.namespace, &args.prefix).await?
    } else {
        let store = JobStorage::for_object_uris().await?;
        let blobs = store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut values = Vec::with_capacity(blobs.len());
        for blob in blobs {
            let object = crate::remote::object_store::ObjectRef::from_storage_path(&blob.name)?;
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
        let mut rows = Vec::with_capacity(values.len());
        for value in &values {
            // A listed object without its URI cannot be named, so it is
            // refused with the row the listing carried, not printed blank.
            let Some(uri) = value["uri"].as_str() else {
                return Err(CmdError::click(format!(
                    "the object listing carries an object without its uri: {value}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            };
            rows.push(vec![
                uri.to_string(),
                value
                    .get("size")
                    .and_then(Value::as_u64)
                    .map_or_else(|| "?".to_string(), |size| size.to_string()),
                match value["updated_at"].as_str() {
                    Some(stamp) => stamp.to_string(),
                    None => "not reported".to_string(),
                },
            ]);
        }
        print_table(&["URI", "BYTES", "UPDATED"], &rows);
    }
    Ok(())
}

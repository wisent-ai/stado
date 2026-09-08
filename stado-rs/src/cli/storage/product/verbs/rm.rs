//! `stado storage rm` and `stado storage url`.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageRmArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageUrlArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    #[arg(long)]
    json: bool,
}

pub(in crate::cli::storage) async fn rm(args: &StorageRmArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    if object.namespace() == "releases" {
        // True of a published release object, and false of the parts staged
        // below it - which is why the refusal names the command that removes
        // those instead of leaving them unreachable.
        return Err(CmdError::click(
            "release objects are immutable and cannot be deleted; to discard the staged parts of \
             an interrupted upload use `stado storage abort-upload <target-uri>`",
        ));
    }
    let uri = object.to_string();
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        remote.delete(&uri).await?;
    } else {
        let store = JobStorage::new().await?;
        store.delete_blob(&object.storage_path()).await?;
    }
    if args.json {
        echo_json(&json!({"state": "absent", "uri": uri}))?;
    } else {
        println!("{uri}");
    }
    Ok(())
}

pub(in crate::cli::storage) fn object_url(args: &StorageUrlArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    let base_url = configured_api_origin()?
        .ok_or_else(|| CmdError::click("STADO_API_URL is required to render an object URL"))?;
    let route = if object.namespace() == "releases" {
        "/api/release/object"
    } else {
        "/api/object"
    };
    let uri = object.to_string();
    let url = object_api_endpoint(&base_url, route, &[("uri", &uri)])?;
    if args.json {
        echo_json(&json!({"uri": uri, "url": url.as_str()}))?;
    } else {
        println!("{url}");
    }
    Ok(())
}

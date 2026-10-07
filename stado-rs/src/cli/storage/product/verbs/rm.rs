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
    let uri = delete_object(&args.uri).await?;
    if args.json {
        echo_json(&json!({"state": "absent", "uri": uri}))?;
    } else {
        println!("{uri}");
    }
    Ok(())
}

/// Delete one object through the route its namespace requires, refusing a
/// release object. `stado storage rm` and `stado release catalog retire`
/// delete through this one implementation.
pub(crate) async fn delete_object(uri: &str) -> Result<String, CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(uri)?;
    if object.namespace() == "releases" {
        // True of a published release object, and false of the parts staged
        // below it - which is why the refusal names the command that removes
        // those instead of leaving them unreachable.
        return Err(CmdError::refused(
            "release objects are immutable and cannot be deleted; to discard the staged parts of \
             an interrupted upload use `stado storage abort-upload <target-uri>`",
        ));
    }
    let uri = object.to_string();
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        remote.delete(&uri).await?;
    } else {
        let store = JobStorage::for_object_uris().await?;
        store.delete_blob(&object.storage_path()).await?;
    }
    Ok(uri)
}

/// The route every release object is publicly read through.
const RELEASE_ROUTE: &str = "/api/release/object";

/// A release object's address is the declared public origin that publishes
/// [`RELEASE_ROUTE`], because that is the only one a client outside this
/// deployment can read without a bearer; the configured API origin is where
/// this machine talks to Stado, which need not forward the route at all.
/// Every other namespace is read through the configured API
/// with a bearer.
pub(in crate::cli::storage) async fn object_url(args: &StorageUrlArgs) -> Result<(), CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(&args.uri)?;
    let (base_url, route) = if object.namespace() == "releases" {
        let document = crate::cli::registry::fetch_document().await?;
        let origin = crate::public_origin::publishing(&document, RELEASE_ROUTE)
            .map_err(CmdError::declaration)?
            .origin();
        let base_url = url::Url::parse(&origin).map_err(|error| {
            CmdError::click(format!("declared public origin {origin}: {error}"))
        })?;
        (base_url, RELEASE_ROUTE)
    } else {
        let base_url = configured_api_origin()?.ok_or_else(|| {
            CmdError::click("STADO_API_URL is required to render an object URL")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        (base_url, "/api/object")
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

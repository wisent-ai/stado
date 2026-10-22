//! `stado storage cp|mv SOURCE TARGET`: copy one product object to another
//! key of its namespace on the server, and with `mv` remove the source once
//! the copy answered.
//!
//! The copy is the S3 endpoint's CopyObject, signed with this client's
//! object API bearer as the namespace's secret, so the bytes never come to
//! this machine and the namespace's grant decides it: `get` on the source
//! and `put` on the target. The target gets the source's metadata. The
//! source is removed through the object API as `stado storage rm` removes
//! it, only after the endpoint answered the copy, so a move that fails part
//! way leaves the source in place.

use crate::cli::storage::*;
use crate::remote::object_store::ObjectRef;
use crate::remote::sigv4;

use super::super::rm::delete_object;
use super::{object_api_origin, refused_by, s3_root, signed};

#[derive(Args, Debug)]
pub struct StorageCopyObjectArgs {
    /// stado://<namespace>/<key> to copy from.
    source: String,
    /// stado://<namespace>/<key> to copy to, in the source's namespace.
    target: String,
    #[arg(long)]
    json: bool,
}

/// The two objects of one copy, refused before anything is asked when they
/// are not two product objects of one namespace.
fn pair(args: &StorageCopyObjectArgs) -> Result<(ObjectRef, ObjectRef), CmdError> {
    let source = ObjectRef::parse(&args.source)?;
    let target = ObjectRef::parse(&args.target)?;
    if source.release_namespace().is_some() || target.release_namespace().is_some() {
        return Err(CmdError::usage(
            "release objects are published with `stado storage put` and never copied or moved; \
             cp and mv take product objects",
        ));
    }
    if source.namespace() != target.namespace() {
        return Err(CmdError::usage(format!(
            "{source} and {target} are in different namespaces; an object is copied within its \
             own namespace, whose secret signs the copy"
        )));
    }
    if source.key() == target.key() {
        return Err(CmdError::usage(format!(
            "{source} is both the source and the target"
        )));
    }
    Ok((source, target))
}

/// Copy `args.source` to `args.target`, then, when `moving`, remove the
/// source.
pub(in crate::cli::storage) async fn copy_object(
    args: &StorageCopyObjectArgs,
    moving: bool,
) -> Result<(), CmdError> {
    let (source, target) = pair(args)?;
    let origin = object_api_origin("copy an object")?;
    let root = s3_root(&origin);
    let mut url = origin.clone();
    url.set_path(&sigv4::object_path(&root, target.namespace(), target.key()));
    let copy_source = sigv4::object_path("", source.namespace(), source.key());
    let carried = [("x-amz-copy-source", copy_source.as_str())];
    let response = signed(reqwest::Method::PUT, &url, target.namespace(), &carried).await?;
    if !response.status().is_success() {
        let what = format!("a CopyObject of {source} to {target}");
        return Err(refused_by(url.as_str(), &what, &response));
    }
    // The copy stands once the endpoint answered it, so a source that cannot
    // be removed leaves the object at both keys, and the failure says so.
    let removed = match moving {
        true => Some(delete_object(&source.to_string()).await.map_err(|error| {
            error.within(format!(
                "{source} was copied to {target}, and removing {source} failed; the object is \
                 at both keys"
            ))
        })?),
        false => None,
    };
    let report = json!({
        "source": source.to_string(),
        "target": target.to_string(),
        "copied": true,
        "removed": removed,
    });
    if args.json {
        return echo_json(&report);
    }
    match removed {
        Some(_) => println!("moved {source} to {target}"),
        None => println!("copied {source} to {target}"),
    }
    Ok(())
}

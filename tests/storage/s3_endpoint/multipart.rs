//! An upload a client leaves unfinished, as the AWS CLI finds and ends it:
//! CreateMultipartUpload and its first part, then ListMultipartUploads names
//! the upload, ListParts answers that one part with the size and the ETag
//! UploadPart gave, and AbortMultipartUpload removes it, after which
//! ListMultipartUploads no longer names it and ListParts is NoSuchUpload.

use serde_json::Value;
use std::fs;
use std::num::NonZeroU32;

use super::deployment::Deployment;
use super::{aws, json_of, NAMESPACE};

const KEY: &str = "probe/abandoned.bin";

/// The upload ids ListMultipartUploads names for [`KEY`].
fn listed_uploads(deployment: &mut Deployment, part: usize) -> Vec<String> {
    let mut list = aws(deployment, part);
    list.args(["s3api", "list-multipart-uploads", "--bucket", NAMESPACE]);
    list.args(["--prefix", "probe/"]);
    let answer = deployment.must("aws s3api list-multipart-uploads", list);
    // The AWS CLI prints nothing when the answer lists no upload.
    if answer.trim().is_empty() {
        return Vec::new();
    }
    let listed = json_of(&answer);
    let uploads = match listed["Uploads"].as_array() {
        Some(uploads) => uploads.clone(),
        None => Vec::new(),
    };
    uploads
        .iter()
        .filter(|upload| upload["Key"] == Value::from(KEY))
        .map(|upload| upload["UploadId"].as_str().unwrap().to_string())
        .collect()
}

pub(super) fn abandoned(deployment: &mut Deployment, part: usize) {
    let mut create = aws(deployment, part);
    create.args(["s3api", "create-multipart-upload", "--bucket", NAMESPACE, "--key", KEY]);
    let created = json_of(&deployment.must("aws s3api create-multipart-upload", create));
    let upload = created["UploadId"].as_str().unwrap().to_string();

    // The first part number the S3 API admits.
    let first = NonZeroU32::MIN.to_string();
    let body = deployment.root.join("abandoned-part.bin");
    let bytes = format!("one part of an upload nobody completes {}", uuid::Uuid::new_v4());
    fs::write(&body, &bytes).unwrap();
    let body_path = body.to_string_lossy().into_owned();
    let mut upload_part = aws(deployment, part);
    upload_part.args(["s3api", "upload-part", "--bucket", NAMESPACE, "--key", KEY]);
    upload_part.args(["--upload-id", &upload, "--part-number", &first, "--body", &body_path]);
    let uploaded = json_of(&deployment.must("aws s3api upload-part", upload_part));
    let etag = uploaded["ETag"].clone();

    let listed = listed_uploads(deployment, part);
    assert_eq!(listed, vec![upload.clone()], "ListMultipartUploads does not name the upload");

    let mut parts = aws(deployment, part);
    parts.args(["s3api", "list-parts", "--bucket", NAMESPACE, "--key", KEY]);
    parts.args(["--upload-id", &upload]);
    let answer = json_of(&deployment.must("aws s3api list-parts", parts));
    let only = match answer["Parts"].as_array().map(Vec::as_slice) {
        Some([only]) => only.clone(),
        other => panic!("ListParts answers other than the one part: {other:?}"),
    };
    assert_eq!(only["PartNumber"].to_string(), first, "{answer}");
    assert_eq!(only["ETag"], etag, "ListParts answers another ETag than UploadPart");
    assert_eq!(only["Size"], Value::from(bytes.len()), "{answer}");

    let mut abort = aws(deployment, part);
    abort.args(["s3api", "abort-multipart-upload", "--bucket", NAMESPACE, "--key", KEY]);
    abort.args(["--upload-id", &upload]);
    deployment.must("aws s3api abort-multipart-upload", abort);
    let after = listed_uploads(deployment, part);
    assert!(after.is_empty(), "an aborted upload is still listed: {after:?}");
    let mut gone = aws(deployment, part);
    gone.args(["s3api", "list-parts", "--bucket", NAMESPACE, "--key", KEY]);
    gone.args(["--upload-id", &upload]);
    let refusal = deployment.refused("aws s3api list-parts (aborted)", gone);
    assert!(refusal.contains("NoSuchUpload"), "{refusal}");
}

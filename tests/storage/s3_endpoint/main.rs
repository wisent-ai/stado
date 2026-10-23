//! Stado as an S3 provider, driven by the real AWS CLI and the real `stado`.
//!
//! One isolated deployment ([`deployment`]) serves one namespace. Signing
//! with the namespace as its access key id and the namespace's object API
//! bearer as its secret, the AWS CLI must put an object (whose bytes then
//! sit at `<store>/ecosystem/<namespace>/<key>`), list it, read it back
//! byte for byte, copy it, upload an object of two parts in parts, copy
//! that object on the server in ranged parts (UploadPartCopy), and delete
//! all four (after which none is in the store); an upload left
//! unfinished is listed by ListMultipartUploads, its part read back by
//! ListParts, and aborted ([`multipart`]). The refusals: a
//! wrong secret is `SignatureDoesNotMatch`, an access key id no namespace
//! answers to is `InvalidAccessKeyId` without naming the served namespaces,
//! a bucket other than the signer's namespace is `AccessDenied`, and
//! turning versioning on is `NotImplemented` while reading it answers the
//! empty configuration of a never-versioned bucket. Then
//! the real `stado` legs run ([`clients`]): `stado storage s3-endpoint`,
//! presigned URLs, `stado storage cp|mv`, a queue kept on the endpoint, and
//! `stado storage stat` of one key through that queue's S3 backend.
//!
//! Run with `cargo test --test storage-s3-endpoint -- --nocapture` and:
//! SKARBIEC (the skarbiec binary under test), STADO_TEST_REQUEST_LIMITS (the
//! qualification API byte bounds), STADO_TEST_S3_PART_BYTES (the part size
//! the AWS CLI uploads in: S3 refuses a part other than the last below its
//! documented minimum, and the request limits must admit one), and
//! STADO_TEST_PRESIGN_SECONDS (the lifetime of the presigned URLs).
mod clients;
mod commands;
mod deployment;
mod multipart;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

const NAMESPACE: &str = "s3-endpoint-probe";
const ITEM: &str = "s3-endpoint-probe-object-api";

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// A whole number the run declares in the environment variable `name`.
fn declared(name: &str) -> usize {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(error) => panic!("{name} must be declared: {error}"),
    };
    match value.parse() {
        Ok(number) => number,
        Err(error) => panic!("{name} {value:?} is not a whole number: {error}"),
    }
}

/// The AWS CLI signing as the namespace with its bearer, uploading in parts
/// of `part` bytes.
fn aws(deployment: &Deployment, part: usize) -> Command {
    deployment.aws(NAMESPACE, &deployment.bearer, part)
}

/// Whether the served store holds `key` with exactly the bytes of `file`.
fn holds(deployment: &Deployment, key: &str, file: &std::path::Path) -> bool {
    let stored = fs::read(deployment.stored(key)).unwrap();
    stored == fs::read(file).unwrap()
}

/// Put, list, read back, copy, upload in parts and delete with the AWS CLI.
fn objects(deployment: &mut Deployment, part: usize) {
    let body = deployment.root.join("small.txt");
    fs::write(
        &body,
        format!("written through s3 {}", uuid::Uuid::new_v4()),
    )
    .unwrap();
    let body_path = body.to_string_lossy().into_owned();
    let mut put = aws(deployment, part);
    put.args(["s3api", "put-object", "--bucket", NAMESPACE]);
    put.args(["--key", "probe/a.txt", "--body", &body_path]);
    deployment.must("aws s3api put-object", put);
    assert!(holds(deployment, "probe/a.txt", &body));

    let mut list = aws(deployment, part);
    list.args(["s3api", "list-objects-v2", "--bucket", NAMESPACE]);
    list.args(["--prefix", "probe/"]);
    let listed = json_of(&deployment.must("aws s3api list-objects-v2", list));
    let contents = listed["Contents"].as_array().unwrap();
    let keys: Vec<&str> = contents
        .iter()
        .map(|entry| entry["Key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, vec!["probe/a.txt"], "{listed}");

    let back = deployment.root.join("back.txt");
    let back_path = back.to_string_lossy().into_owned();
    let mut get = aws(deployment, part);
    get.args(["s3api", "get-object", "--bucket", NAMESPACE]);
    get.args(["--key", "probe/a.txt", &back_path]);
    deployment.must("aws s3api get-object", get);
    assert_eq!(fs::read(&back).unwrap(), fs::read(&body).unwrap());

    let source = format!("{NAMESPACE}/probe/a.txt");
    let mut copy = aws(deployment, part);
    copy.args(["s3api", "copy-object", "--bucket", NAMESPACE]);
    copy.args(["--key", "probe/b.txt", "--copy-source", &source]);
    deployment.must("aws s3api copy-object", copy);
    assert!(holds(deployment, "probe/b.txt", &body));

    // Two parts of distinct random bytes, so parts assembled out of order
    // or one part written twice would not compare equal.
    let large = deployment.root.join("large.bin");
    let random = std::iter::repeat_with(|| uuid::Uuid::new_v4().into_bytes());
    let bytes: Vec<u8> = random.flatten().take(part + part).collect();
    fs::write(&large, &bytes).unwrap();
    let large_key = "probe/large.bin";
    let target = format!("s3://{NAMESPACE}/{large_key}");
    let mut cp = aws(deployment, part);
    cp.args(["s3", "cp", &large.to_string_lossy(), &target]);
    deployment.must("aws s3 cp (in parts)", cp);
    let assembled = fs::read(deployment.stored(large_key)).unwrap();
    assert!(assembled == bytes, "the parts were not assembled in order");

    // Above the multipart threshold the AWS CLI copies on the server in
    // ranged parts (UploadPartCopy); each part must carry its range's bytes.
    let copy_key = "probe/large-copy.bin";
    let copied = format!("s3://{NAMESPACE}/{copy_key}");
    let mut server_copy = aws(deployment, part);
    server_copy.args(["s3", "cp", &target, &copied]);
    deployment.must("aws s3 cp (server-side, in parts)", server_copy);
    let assembled = fs::read(deployment.stored(copy_key)).unwrap();
    assert!(
        assembled == bytes,
        "the copied parts do not hold the source's bytes in order"
    );

    let mut written: Vec<&str> = Vec::new();
    written.push("probe/a.txt");
    written.push("probe/b.txt");
    written.push(large_key);
    written.push(copy_key);
    for key in written {
        let mut delete = aws(deployment, part);
        delete.args([
            "s3api",
            "delete-object",
            "--bucket",
            NAMESPACE,
            "--key",
            key,
        ]);
        deployment.must("aws s3api delete-object", delete);
        assert!(
            !deployment.stored(key).exists(),
            "{key} is still in the store after DeleteObject"
        );
    }
}

/// A wrong secret, an unknown access key id and another bucket are refused,
/// and versioning reads as never enabled and cannot be turned on.
fn refusals(deployment: &mut Deployment, part: usize) {
    let mut forged = deployment.aws(NAMESPACE, "not-the-bearer", part);
    forged.args(["s3api", "list-objects-v2", "--bucket", NAMESPACE]);
    let refusal = deployment.refused("aws s3api list-objects-v2 (wrong secret)", forged);
    assert!(
        refusal.contains("SignatureDoesNotMatch"),
        "a wrong secret: {refusal}"
    );

    let bearer = deployment.bearer.clone();
    let mut stranger = deployment.aws("no-such-namespace", &bearer, part);
    stranger.args(["s3api", "list-objects-v2", "--bucket", "no-such-namespace"]);
    let refusal = deployment.refused("aws s3api list-objects-v2 (unknown access key)", stranger);
    assert!(
        refusal.contains("InvalidAccessKeyId"),
        "an unknown access key id: {refusal}"
    );
    let named = refusal.contains(NAMESPACE);
    assert!(
        !named,
        "the refusal named a served namespace to an unsigned caller: {refusal}"
    );

    let mut other = aws(deployment, part);
    other.args(["s3api", "list-objects-v2", "--bucket", "another-bucket"]);
    let refusal = deployment.refused("aws s3api list-objects-v2 (another bucket)", other);
    assert!(
        refusal.contains("AccessDenied"),
        "another bucket: {refusal}"
    );

    // Objects keep no versions: the bucket reads as never versioned (the
    // AWS CLI prints nothing for that empty configuration), and turning
    // versioning on is NotImplemented, not taken for a CreateBucket.
    let mut read = aws(deployment, part);
    read.args(["s3api", "get-bucket-versioning", "--bucket", NAMESPACE]);
    let answer = deployment.must("aws s3api get-bucket-versioning", read);
    assert!(
        answer.trim().is_empty(),
        "a bucket that keeps no versions: {answer}"
    );
    let mut enable = aws(deployment, part);
    enable.args(["s3api", "put-bucket-versioning", "--bucket", NAMESPACE]);
    enable.args(["--versioning-configuration", "Status=Enabled"]);
    let refusal = deployment.refused("aws s3api put-bucket-versioning", enable);
    assert!(
        refusal.contains("NotImplemented"),
        "turning versioning on: {refusal}"
    );
}

#[tokio::test]
async fn the_aws_cli_reads_and_writes_a_namespace_through_stados_s3_endpoint() {
    let part = declared("STADO_TEST_S3_PART_BYTES");
    let lifetime = declared("STADO_TEST_PRESIGN_SECONDS").to_string();
    let mut deployment = Deployment::start(NAMESPACE, ITEM);
    objects(&mut deployment, part);
    multipart::abandoned(&mut deployment, part);
    refusals(&mut deployment, part);
    clients::endpoint(&mut deployment);
    clients::presigned_and_copied(&mut deployment, &lifetime).await;
    clients::queue(&mut deployment);
    clients::stat(&mut deployment, part);
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}

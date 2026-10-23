//! The real `stado` against the deployment's S3 endpoint: `stado storage
//! s3-endpoint` reports the endpoint after its signed HeadBucket answered
//! 200; a URL from `stado storage url --presign put` writes an object with
//! no bearer, one from `--presign get` reads it back, and the same URL moved
//! to another key is refused; `stado storage cp` copies the object on the
//! server and keeps the source, `mv` copies and removes it, a copy to
//! another namespace is refused before anything is asked and one from an
//! absent key by the endpoint, the target left as it was; and a Stado
//! keeping its queue on the endpoint — as the named provider `minio` and as
//! `s3-compatible`, its key pair read from the vault by role — submits a job
//! whose record lands in the served store, after a conditional-write proof
//! that leaves no object behind.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::deployment::Deployment;
use super::{json_of, NAMESPACE};

pub(super) fn endpoint(deployment: &mut Deployment) {
    let mut endpoint = deployment.stado();
    endpoint.args(["storage", "s3-endpoint", NAMESPACE, "--json"]);
    let described = json_of(&deployment.must("stado storage s3-endpoint", endpoint));
    let ok = json!(reqwest::StatusCode::OK.as_u16());
    assert_eq!(described["check"]["status"], ok, "{described}");
    let expected = json!(format!("{}/s3", deployment.origin));
    assert_eq!(described["endpoint"], expected, "{described}");
    assert_eq!(described["access_key_id"], json!(NAMESPACE));
}

/// The URL `stado storage url --presign METHOD` prints for `uri`.
fn presigned(deployment: &mut Deployment, uri: &str, method: &str, lifetime: &str) -> String {
    let mut command = deployment.stado();
    command.args(["storage", "url", uri, "--presign", method]);
    command.args(["--expires", lifetime, "--json"]);
    let label = format!("stado storage url --presign {method}");
    let answer = json_of(&deployment.must(&label, command));
    answer["url"].as_str().unwrap().to_string()
}

fn text(deployment: &Deployment, key: &str) -> String {
    fs::read_to_string(deployment.stored(key)).unwrap()
}

pub(super) async fn presigned_and_copied(deployment: &mut Deployment, lifetime: &str) {
    let uri = format!("stado://{NAMESPACE}/probe/presigned.txt");
    let words = format!("written through a presigned URL {}", uuid::Uuid::new_v4());
    let client = reqwest::Client::new();
    let put_url = presigned(deployment, &uri, "put", lifetime);
    let written = client
        .put(&put_url)
        .body(words.clone())
        .send()
        .await
        .unwrap();
    assert!(
        written.status().is_success(),
        "a presigned PUT: {}",
        written.status()
    );
    assert_eq!(text(deployment, "probe/presigned.txt"), words);
    let get_url = presigned(deployment, &uri, "get", lifetime);
    let read = client.get(&get_url).send().await.unwrap();
    assert!(
        read.status().is_success(),
        "a presigned GET: {}",
        read.status()
    );
    assert_eq!(read.text().await.unwrap(), words);
    let tampered = get_url.replace("presigned.txt", "other.txt");
    let refused = client.get(&tampered).send().await.unwrap();
    let forbidden = reqwest::StatusCode::FORBIDDEN;
    assert_eq!(
        refused.status(),
        forbidden,
        "a presigned URL for another key"
    );

    let copied = format!("stado://{NAMESPACE}/probe/copied.txt");
    let moved = format!("stado://{NAMESPACE}/probe/moved.txt");
    let mut cp = deployment.stado();
    cp.args(["storage", "cp", &uri, &copied, "--json"]);
    let answer = json_of(&deployment.must("stado storage cp", cp));
    assert_eq!(answer["removed"], Value::Null, "{answer}");
    assert_eq!(text(deployment, "probe/copied.txt"), words);
    assert!(
        deployment.stored("probe/presigned.txt").exists(),
        "cp removed its source"
    );
    let mut mv = deployment.stado();
    mv.args(["storage", "mv", &copied, &moved, "--json"]);
    let answer = json_of(&deployment.must("stado storage mv", mv));
    assert_eq!(answer["removed"], json!(copied), "{answer}");
    assert_eq!(text(deployment, "probe/moved.txt"), words);
    assert!(
        !deployment.stored("probe/copied.txt").exists(),
        "mv left its source"
    );

    let mut across = deployment.stado();
    across.args([
        "storage",
        "cp",
        &moved,
        "stado://another-namespace/probe/x.txt",
    ]);
    let refusal = deployment.refused("stado storage cp (another namespace)", across);
    assert!(refusal.contains("different namespaces"), "{refusal}");
    let absent = format!("stado://{NAMESPACE}/probe/absent.txt");
    let mut missing = deployment.stado();
    missing.args(["storage", "mv", &absent, &moved]);
    let refusal = deployment.refused("stado storage mv (absent source)", missing);
    assert!(refusal.contains("NoSuchKey"), "{refusal}");
    assert_eq!(text(deployment, "probe/moved.txt"), words);
}

/// Every file under `root`; none when `root` does not exist.
fn stored_files(root: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("{} cannot be listed: {error}", root.display()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(stored_files(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// Whether the file at `path` holds `needle`'s bytes.
fn contains(path: &Path, needle: &str) -> bool {
    let bytes = fs::read(path).unwrap();
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

pub(super) fn queue(deployment: &mut Deployment) {
    let namespace = deployment.store().join("ecosystem").join(NAMESPACE);
    for backend in ["minio", "s3-compatible"] {
        let run = format!("s3-endpoint-{backend}-{}", uuid::Uuid::new_v4().simple());
        let mut submit = deployment.queue_client(backend);
        submit.args(["submit", "--run-id", &run, "true"]);
        deployment.must(&format!("stado submit (storage.backend {backend})"), submit);
        let held = stored_files(&namespace)
            .iter()
            .any(|path| contains(path, &run));
        assert!(
            held,
            "no object in the served store holds the job submitted through {backend}"
        );
    }
    let proofs = namespace.join("system/conditional-writes");
    let left = stored_files(&proofs);
    assert!(
        left.is_empty(),
        "the conditional-write proof left objects behind: {left:?}"
    );
}

/// `stado storage stat` with its queue on the endpoint (the S3 backend, one
/// HEAD per object): two keys sharing a prefix are put through the AWS CLI
/// with bodies and metadata of their own; stat of the shorter key answers
/// its own size and metadata, never the longer one's, and a key that only
/// prefixes both is absent with no metadata.
pub(super) fn stat(deployment: &mut Deployment, part: usize) {
    let mut put = |key: &str, body: &str, kind: &str| {
        let file = deployment.root.join(format!("stat-{kind}.txt"));
        fs::write(&file, body).unwrap();
        let path = file.to_string_lossy().into_owned();
        let mut command = super::aws(deployment, part);
        command.args(["s3api", "put-object", "--bucket", NAMESPACE]);
        command.args(["--key", key, "--body", &path]);
        command.args(["--metadata", &format!("kind={kind}")]);
        deployment.must("aws s3api put-object (stat)", command);
    };
    let short = "short body\n";
    put("stat/blob", short, "short");
    put(
        "stat/blob-longer",
        "a longer body than the short one\n",
        "long",
    );

    let mut stat = |key: &str| {
        let mut command = deployment.queue_client("s3-compatible");
        command.args(["storage", "stat", key, "--json"]);
        json_of(&deployment.must("stado storage stat (S3 backend)", command))
    };
    let named = stat("stat/blob");
    assert_eq!(named["state"], "present", "{named}");
    assert_eq!(named["size"], json!(short.len()), "its own size: {named}");
    assert_eq!(
        named["metadata"]["kind"], "short",
        "its own metadata: {named}"
    );
    assert_eq!(named["metadata_error"], Value::Null, "{named}");
    let prefix = stat("stat/bl");
    assert_eq!(
        prefix["state"], "absent",
        "a key that only prefixes objects: {prefix}"
    );
    assert_eq!(prefix["metadata"], json!({}), "{prefix}");
}

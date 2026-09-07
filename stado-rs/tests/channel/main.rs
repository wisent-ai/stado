//! Public release-channel contract through the real production ingress.
//!
//! This is deliberately ignored by a plain `cargo test`: it needs the public
//! control origin and one immutable release coordinate. The repository's test
//! runner can supply both directly. No stand-in server, loopback override, dry
//! run, or fixture replaces the channel. The built Stado binary performs every
//! network operation; the test retains, verifies and executes the fetched bytes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::read::GzDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Archive;

const PRODUCT: &str = "stado";

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} is required by the public channel journey"))
}

fn retain_command(home: &Path, executable: &Path, args: &[&str], output: &Output) {
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("commands.jsonl"))
        .expect("open retained command evidence");
    let receipt = json!({
        "source_revision": env!("STADO_SOURCE_REVISION"),
        "executable": executable,
        "args": args,
        "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    serde_json::to_writer(&mut log, &receipt).expect("retain command evidence");
    writeln!(log).expect("finish command evidence");
}

#[rustfmt::skip]
fn retained(prefix: &str) -> PathBuf {
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/channel");
    fs::create_dir_all(&evidence).expect("create retained channel evidence root");
    let work = tempfile::Builder::new().prefix(prefix).tempdir_in(evidence)
        .expect("create retained channel journey").keep();
    eprintln!("retained channel evidence: {}", work.display());
    work
}

#[rustfmt::skip]
fn run(home: &Path, extra: &[(&str, &str)], args: &[&str]) -> Output {
    let executable = Path::new(env!("CARGO_BIN_EXE_stado"));
    let mut command = Command::new(executable);
    command.env_clear().env("HOME", home)
        .env("PATH", std::env::var("PATH").expect("PATH exists"))
        .env("STADO_CONFIG", home.join("nonexistent-config.json"));
    for (name, value) in extra {
        command.env(name, value);
    }
    let output = command.args(args).output().expect("the built stado binary starts");
    retain_command(home, executable, args, &output);
    output
}

fn stado(home: &Path, origin: &str, args: &[&str]) -> Output {
    run(home, &[("STADO_API_URL", origin)], args)
}

#[rustfmt::skip]
fn isolated(home: &Path, store: &Path, args: &[&str]) -> Output {
    let store = store.to_str().expect("store path is UTF-8");
    run(home, &[("WC_STORAGE_BACKEND", "local"), ("WC_LOCAL_STORAGE_PATH", store)], args)
}

fn successful(out: Output, operation: &str) -> Vec<u8> {
    assert!(
        out.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out.stdout
}

fn uri(version: &str, platform: &str, object: &str) -> String {
    format!("stado://releases/{PRODUCT}/{version}/{platform}/{object}")
}

fn get(home: &Path, origin: &str, object_uri: &str, destination: &Path) {
    let destination = destination.to_str().expect("temporary path is UTF-8");
    successful(
        stado(home, origin, &["storage", "get", object_uri, destination]),
        &format!("storage get {object_uri}"),
    );
}

/// The SIGNED manifest `release.json`, the commit marker written last.
fn manifest(path: &Path, version: &str, platform: &str) -> Value {
    let value: Value = serde_json::from_slice(&fs::read(path).expect("manifest was downloaded"))
        .expect("release manifest is JSON");
    assert_eq!(value["product"], PRODUCT);
    assert_eq!(value["version"], version);
    assert_eq!(value["platform"], platform);
    let digest = value["artifact_sha256"]
        .as_str()
        .expect("manifest artifact_sha256 is a string");
    assert_eq!(digest.len(), 64, "manifest digest has 64 hex digits");
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let revision = value["source_revision"]
        .as_str()
        .expect("manifest source_revision is a string");
    assert!(matches!(revision.len(), 40 | 64));
    assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
    value
}

fn release_binary(archive: &Path, destination: &Path) -> PathBuf {
    Archive::new(GzDecoder::new(
        fs::File::open(archive).expect("release archive reopens"),
    ))
    .unpack(destination)
    .expect("verified release archive extracts");
    let binary = destination.join("stado");
    assert!(
        binary.is_file(),
        "release archive contains the stado binary"
    );
    binary
}

#[test]
#[ignore = "requires a real public release origin and immutable coordinate"]
fn public_release_channel_serves_a_verified_executable_native_release() {
    let origin = required("STADO_RELEASE_CHANNEL_URL");
    assert!(
        origin.starts_with("https://")
            && !origin.contains("localhost")
            && !origin.contains("127.0.0.1"),
        "the release-channel journey must use public HTTPS, got {origin}",
    );
    let version = required("STADO_RELEASE_CHANNEL_VERSION");
    let platform = required("STADO_RELEASE_CHANNEL_PLATFORM");
    let work = retained("channel-");
    let home = work.join("home");
    fs::create_dir_all(&home).expect("temporary HOME exists");

    let manifest_uri = uri(&version, &platform, "release.json");
    let stat = successful(
        stado(
            &home,
            &origin,
            &["storage", "stat", &manifest_uri, "--json"],
        ),
        &format!("storage stat {manifest_uri}"),
    );
    let presence: Value = serde_json::from_slice(&stat).expect("storage stat emits JSON");
    assert_eq!(
        presence["state"], "present",
        "the public channel must testify that the manifest is present: {presence}",
    );

    let manifest_path = work.join("release.json");
    get(&home, &origin, &manifest_uri, &manifest_path);
    let manifest = manifest(&manifest_path, &version, &platform);

    let archive_uri = uri(&version, &platform, "release.tar.gz");
    let archive_path = work.join("release.tar.gz");
    get(&home, &origin, &archive_uri, &archive_path);
    let archive_bytes = fs::read(&archive_path).expect("release archive was downloaded");
    let actual_digest = hex::encode(Sha256::digest(&archive_bytes));
    assert_eq!(
        actual_digest,
        manifest["artifact_sha256"].as_str().unwrap(),
        "the public archive bytes match the downloaded release manifest",
    );
    assert_eq!(
        archive_bytes.len() as u64,
        manifest["artifact_bytes"].as_u64().unwrap(),
        "the public archive is the size its manifest binds",
    );

    let extracted = work.join("extracted");
    fs::create_dir(&extracted).expect("extract directory exists");
    let released_stado = release_binary(&archive_path, &extracted);
    let version_out = Command::new(&released_stado)
        .arg("--version")
        .output()
        .expect("the released native binary executes");
    retain_command(&home, &released_stado, &["--version"], &version_out);
    assert!(
        version_out.status.success(),
        "released binary --version failed: {}",
        String::from_utf8_lossy(&version_out.stderr),
    );
    let reported = String::from_utf8(version_out.stdout).expect("version output is UTF-8");
    assert!(
        reported.contains(&version),
        "released binary reports {reported:?}, expected version {version}",
    );
    println!(
        "verified {manifest_uri}; archive={archive_uri}; sha256={actual_digest}; binary={}",
        released_stado.display(),
    );
}

#[rustfmt::skip]
fn one_target() -> Value {
    json!({
        "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [{"name": "macbook-fake", "kind": "local",
            "ssh": "operator@edge.example.com", "release_platform": "darwin-arm64",
            "hostnames": ["macbook-fake.local"]}],
    })
}

#[rustfmt::skip]
fn derived_origin() -> Value {
    json!([{
        "name": "release-object", "hostname": "edge.example.com",
        "target": "macbook-fake", "publication": "tailscale-funnel",
        "upstream": "http://127.0.0.1:8765", "paths": ["/api/release/object"],
    }])
}

#[rustfmt::skip]
const DECLARE_UNRESOLVABLE: &[&str] = &[
    "web", "origin", "declare", "release-object",
    "--hostname", "release-origin-does-not-exist.invalid",
    "--target", "macbook-fake", "--upstream", "http://127.0.0.1:8765",
    "--path", "/api/release/object", "--json",
];

fn body(document: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(document).expect("serialize the document")
}

fn seeded() -> (PathBuf, PathBuf) {
    let work = retained("public-origin-");
    let (home, store) = (work.join("home"), work.join("store"));
    fs::create_dir_all(&home).expect("temporary HOME exists");
    fs::create_dir_all(&store).expect("isolated canonical store exists");
    let document = work.join("registry.json");
    fs::write(&document, body(&one_target())).expect("seed document is written");
    let seed = ["registry", "push", document.to_str().expect("UTF-8 path")];
    successful(isolated(&home, &store, &seed), "registry push");
    (home, store)
}

/// A public origin no public resolver can answer is refused before it is
/// written; `.invalid` is reserved by RFC 2606 so it can never resolve.
#[test]
fn declaring_a_public_origin_with_no_public_record_is_refused_and_writes_nothing() {
    let (home, store) = seeded();
    let refused = isolated(&home, &store, DECLARE_UNRESOLVABLE);
    assert_eq!(refused.status.code(), Some(1));
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(
        complaint.contains(
            "refusing to declare public origin \"release-object\": \
             release-origin-does-not-exist.invalid has no public A or AAAA \
             record, so no public edge could fetch it; publish the name first, \
             then declare it"
        ),
        "the refusal must name the origin, the hostname and the repair: {complaint}"
    );
    let listed = successful(
        isolated(&home, &store, &["web", "origin", "list", "--json"]),
        "web origin list",
    );
    let rows: Value = serde_json::from_slice(&listed).expect("list emits JSON");
    assert_eq!(rows, json!([]), "a refused declaration leaves no row");
}

/// `/docs/channels`: a control route and a public origin are separate choices.
#[test]
fn a_public_origin_on_a_host_control_destination_is_refused_by_validation() {
    let (home, store) = seeded();
    let mut document = one_target();
    document["public_origins"] = derived_origin();
    let path = home.join("derived.json");
    fs::write(&path, body(&document)).expect("the document is written");
    let argv = ["registry", "validate", path.to_str().expect("UTF-8 path")];
    let refused = isolated(&home, &store, &argv);
    assert!(!refused.status.success());
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(
        complaint.contains(
            "edge.example.com is a declared host-control destination; a public \
             origin is a separate choice from the route Stado reaches the host \
             on and must not be derived from it"
        ),
        "validation must name the derivation it refuses: {complaint}"
    );
}

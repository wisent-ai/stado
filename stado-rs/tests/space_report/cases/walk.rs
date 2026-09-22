//! What the walk finds, and what it says about what it could not open.

use std::fs;

use crate::fixture::Fixture;

/// `permission-denied` row while the tagged cache beside it is still found.
#[test]
fn an_unreadable_directory_is_one_row_and_a_refused_root_is_never_opened() {
    let fixture = Fixture::new();
    let cloud = fixture.home.join("Library/CloudStorage/drive/.tmp");
    let secret = fixture.home.join("secret");
    let cache = fixture.home.join("work/target");
    for directory in [&cloud, &secret, &cache] {
        fs::create_dir_all(directory).expect("create fixture directory");
    }
    fs::write(
        cache.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write cache tag");
    for closed in [&cloud, &secret] {
        let mut permissions = fs::metadata(closed)
            .expect("stat closed directory")
            .permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(closed, permissions).expect("close fixture directory");
    }

    let output = fixture.report("0", &["--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    for closed in [&cloud, &secret] {
        let mut permissions = fs::metadata(closed)
            .expect("stat closed directory")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(closed, permissions).expect("reopen fixture directory");
    }
    assert_eq!(
        output.status.code(),
        Some(0),
        "one unreadable directory failed the whole host; stderr: {stderr}"
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let caches = &document["build_caches"];
    assert!(
        caches["error"].is_null(),
        "the verdict carried an error instead of rows: {}",
        caches["error"]
    );
    let entries = caches["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("the verdict lists its rows: {document}"));
    let state_of = |path: &Path| {
        entries
            .iter()
            .find(|entry| entry["path"].as_str() == Some(path.to_str().unwrap()))
            .map(|entry| entry["verdict"].as_str().unwrap_or_default().to_string())
    };
    assert_eq!(
        state_of(&secret).as_deref(),
        Some("permission-denied"),
        "the unreadable directory outside the refused roots is its own row: {entries:?}"
    );
    assert!(
        state_of(&cache).is_some_and(|state| state != "scan-failed"),
        "the tagged cache beside the unreadable directory was still found: {entries:?}"
    );
    assert!(
        entries.iter().all(|entry| !entry["path"]
            .as_str()
            .unwrap_or_default()
            .contains("CloudStorage")),
        "the refused root was opened: {entries:?}"
    );
    assert!(
        !stderr.contains("credentials this command used were rejected"),
        "a file the host would not open was reported as rejected credentials: {stderr}"
    );
    fixture.cleanup();
}

/// The inventory walks `$HOME` two levels deep, so build output any deeper
/// than that was invisible to the coverage report: on 2026-09-19
/// `lukasz-macbook` held 843 GB of tagged `target/` trees four and five
/// levels down, the host declared the `build_caches` cleaner all along, the
/// cleaner's root reached none of them, and every reading said the host was
/// fine while the volume stood at 97%.
///
/// The census finds a tagged tree wherever it is, and the existing partition
/// then says what no declared mechanism reaches. Both halves are asserted
/// here: the deep tree is measured at all, and it is named as bytes nothing
/// sweeps.
#[test]
fn build_output_deeper_than_the_walk_is_measured_and_named_as_unswept() {
    let fixture = Fixture::new();
    let deep = fixture
        .home
        .join("Documents/CodingProjects/Wisent/product/target");
    fs::create_dir_all(&deep).expect("create the deep build tree");
    fs::write(
        deep.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write cache tag");
    fs::write(deep.join("artifact.bin"), vec![0_u8; 512 * 1024]).expect("write build output");

    let output = fixture.report("120", &["--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");

    let wanted = deep.to_str().expect("the fixture path is utf-8");
    let measured = document["inventory"]
        .as_array()
        .expect("the report carries an inventory")
        .iter()
        .find(|row| row["path"].as_str() == Some(wanted));
    let measured = measured.unwrap_or_else(|| {
        panic!(
            "the tagged tree four levels below home was never measured: {}",
            document["inventory"]
        )
    });
    assert!(
        measured["bytes"].as_i64().unwrap_or_default() > 0,
        "the tagged tree was measured at zero bytes: {measured}"
    );

    let coverage = &document["coverage"];
    let named = coverage["uncovered"]
        .as_array()
        .expect("the coverage section lists what nothing covers")
        .iter()
        .any(|row| {
            row["path"]
                .as_str()
                .is_some_and(|path| path == wanted || wanted.starts_with(&format!("{path}/")))
        });
    assert!(
        named,
        "build output no declared cleaner reaches was not named: {coverage}"
    );
    assert!(
        coverage["unswept_bytes"].as_i64().unwrap_or_default() > 0,
        "bytes nothing sweeps were reported as none: {coverage}"
    );
    fixture.cleanup();
}

/// A declared cleaner that reaches none of the measured build output says so,
/// and names the root that would reach it.
///
/// This is the half the census alone does not answer. `lukasz-macbook`
/// declared `build_caches` for months, the cleaner covered the root it was
/// pointed at, and the operator's tagged trees were somewhere else entirely;

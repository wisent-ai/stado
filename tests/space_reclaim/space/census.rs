//! The build-cache census is the one walk `space report` and the janitor's
//! `build_caches` cleaner share. It is read here as the product reads it for
//! this host — the local target, the production runner, the program under
//! `/bin/sh` — against a tagged directory in the isolated home: the walk must
//! size that directory, and nothing the shell or `find` prints about its own
//! failure may come back as a directory of the census.

use stado::deploy::host_build_caches::{report_declaration_on_host, BuildCacheDeclaration};
use stado::deploy::production_runner;
use stado::deploy::service::local_target;

use crate::native::Native;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn the_census_sizes_a_tagged_directory_and_keeps_no_error_line_as_a_path() {
    let native = Native::new("space-census");
    let cache = native.home.join("project/target");
    native.cache(&cache);
    let root = native.home.to_str().unwrap().to_string();
    let target = local_target().expect("this host is a target");
    let declaration = BuildCacheDeclaration { root: root.clone() };
    let report = report_declaration_on_host(&target, &declaration, &production_runner()).await;
    let rows: Vec<String> = report
        .entries
        .iter()
        .map(|entry| format!("{} {} {}", entry.state, entry.path, entry.kib))
        .collect();
    native.observe(
        "build-cache census",
        serde_json::json!({
            "error": report.error,
            "timed_out": report.timed_out,
            "entries": rows,
        }),
    );
    assert!(
        report.error.is_none(),
        "the census failed: {}",
        report.error.unwrap()
    );
    let foreign: Vec<&String> = rows.iter().filter(|row| !row.contains(&root)).collect();
    assert!(
        foreign.is_empty(),
        "the census reported paths outside the scanned home: {foreign:?}"
    );
    let sized = report
        .entries
        .iter()
        .find(|entry| entry.path == cache.to_str().unwrap());
    assert!(
        sized.is_some(),
        "the tagged directory {} is missing from the census: {rows:?}",
        cache.display()
    );
    let sized = sized.unwrap();
    assert!(
        sized.kib.parse::<i64>().is_ok(),
        "the tagged directory was not sized: {} {} {}",
        sized.state,
        sized.path,
        sized.kib
    );
}

#[tokio::test]
async fn the_census_preserves_a_real_directory_refusal_and_its_cache_bytes() {
    let native = Native::new("space-census-unreadable");
    let project = native.home.join("unreadable-project");
    let cache = project.join("target");
    native.cache(&cache);
    let payload = cache.join("payload");
    let before = fs::read(&payload).expect("read isolated cache before census");
    let permissions = fs::metadata(&project)
        .expect("read project metadata")
        .permissions();
    let denied = permissions.mode() & !(libc::S_IRWXU | libc::S_IRWXG | libc::S_IRWXO);
    let target = local_target().expect("this host is a target");
    let declaration = BuildCacheDeclaration {
        root: native
            .home
            .to_str()
            .expect("fixture path is UTF-8")
            .to_owned(),
    };
    fs::set_permissions(&project, fs::Permissions::from_mode(denied))
        .expect("deny access to isolated project");
    let access_error = fs::read_dir(&project).err().map(|error| error.to_string());
    let report = report_declaration_on_host(&target, &declaration, &production_runner()).await;
    fs::set_permissions(&project, permissions).expect("restore isolated project permissions");
    native.observe(
        "unreadable build-cache census",
        serde_json::json!({
            "access_error": access_error,
            "error": report.error,
            "entries": report.entries.iter().map(|entry| serde_json::json!({
                "state": entry.state, "path": entry.path, "kib": entry.kib,
            })).collect::<Vec<_>>(),
        }),
    );
    assert!(
        access_error.is_some(),
        "fixture account must encounter a real access refusal"
    );
    assert!(
        report
            .entries
            .iter()
            .any(|entry| entry.state == "permission-denied"
                && entry.path == project.to_str().expect("fixture path is UTF-8")),
        "the census must retain the refused directory"
    );
    assert!(
        !report
            .entries
            .iter()
            .any(|entry| entry.path == cache.to_str().expect("fixture path is UTF-8")),
        "an unreadable cache must not be reported as inspected"
    );
    assert_eq!(fs::read(&payload).expect("read retained cache"), before);
}

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

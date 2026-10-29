//! At the disk-full rule on a host its registry entry declares shared (a
//! person's own computer): only Stado's own tree is cleaned, and every
//! other cache the pass would have taken is measured, kept and counted under
//! `shared_host_not_stado_owned`.

use std::fs;

use serde_json::json;

use super::Native;

/// The skip reason a shared host's kept candidate is counted under, as the
/// shared-host page states it:
/// https://stado.wisent.com/docs/capabilities/space/shared-hosts
const NOT_STADO_OWNED: &str = "shared_host_not_stado_owned";

#[test]
fn at_the_threshold_a_shared_host_cleans_only_stados_own_tree() {
    let native = Native::new("shared-host");
    native.success(&[
        "registry",
        "set",
        "--path",
        "targets.0.ownership",
        "--value",
        "shared",
        "--json",
    ]);
    let stado_cache = native.home.join(".stado/build-cache/host-build");
    native.cache(&stado_cache);
    let project_cache = native.home.join("Projects/her-app/target");
    native.cache(&project_cache);
    let model = native
        .home
        .join(".cache/huggingface/hub/models--example--model/blobs/weights");
    fs::create_dir_all(model.parent().unwrap()).unwrap();
    fs::write(&model, b"her model download").unwrap();
    let log = native.home.join(".omp/logs/request.json");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"{}").unwrap();
    native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    let cleaners = &report["cleaners"];
    assert!(
        !stado_cache.exists(),
        "Stado's own cache survived the rule: {report}"
    );
    assert_eq!(
        cleaners["build_caches"]["deleted_items"],
        json!(1),
        "{report}"
    );
    assert_eq!(
        cleaners["build_caches"]["skipped"][NOT_STADO_OWNED],
        json!(1),
        "{report}"
    );
    assert!(
        project_cache.is_dir(),
        "her project's build tree was deleted: {report}"
    );
    assert_eq!(fs::read(&model).unwrap(), b"her model download");
    assert_eq!(fs::read(&log).unwrap(), b"{}");
    for cleaner in [
        "huggingface_cache",
        "agent_logs",
        "compiler_cache",
        "local_snapshots",
    ] {
        assert_eq!(
            cleaners[cleaner]["skipped"][NOT_STADO_OWNED],
            json!(1),
            "{cleaner} did not say why it kept its candidates: {report}"
        );
        assert_eq!(
            cleaners[cleaner]["deleted_items"],
            json!(0),
            "{cleaner}: {report}"
        );
    }
}

//! The registry authority's `stado resolver snapshot` answers from the
//! generation its running resolver holds, without reading the registry store.
//!
//! One isolated deployment, as in `tests/resolver/unserved.rs`: a local store
//! holding the shared directory template with this machine as `marker-host`,
//! which is also the directory's authority, and the real `stado serve
//! --resolver`. Expected: the snapshot prints the imported generation; with
//! the store moved away it still prints it, because nothing reads the store;
//! with the resolver stopped it is refused at once, naming the process that
//! is not running; and with a published resolver whose API accepts the
//! request and answers nothing it ends by itself at the published refresh
//! interval, naming that API. Read from the store itself, the snapshot on an
//! authority whose store stands would hold every host's registry read behind
//! it; asked of the resolver with no measure, every reader that gives up over
//! SSH would leave one snapshot process waiting on the authority.
//! The same deployment carries `stado resolver follow --generation N`: refused
//! while no resolver published its state; serving, the held generation ends it
//! at once, a higher one is refused with the resolver's own answer, and a
//! registry push is what the next snapshot answers; stopped, it is refused.
#[path = "held_snapshot/deployment.rs"]
mod deployment;

use serde_json::{json, Value};

use deployment::Deployment;

#[test]
fn the_authority_snapshot_answers_from_the_running_resolver_and_never_reads_the_store() {
    let mut deployment = Deployment::start();
    deployment.cli("config-init", &["config", "init"]);
    let api_bind = deployment.free_bind();
    let generation = deployment.import_registry(&api_bind);
    deployment.serve();

    assert_eq!(
        deployment.snapshot_generation("snapshot-served"),
        generation,
        "the snapshot prints the generation the resolver holds"
    );

    // No registry is where the store is declared now: a snapshot that still
    // answers came from the resolver's memory.
    deployment.move_store("store", "store-moved");
    let moved = deployment.run("snapshot-store-moved", &["resolver", "snapshot"]);
    deployment.move_store("store-moved", "store");
    assert!(
        moved.status.success(),
        "the snapshot answers while the store is gone: {}",
        String::from_utf8_lossy(&moved.stderr)
    );
    assert_eq!(
        deployment.snapshot_generation("snapshot-store-back"),
        generation,
        "the snapshot still prints the held generation"
    );

    let pid = deployment.stop();
    let refused = deployment.run("snapshot-stopped", &["resolver", "snapshot"]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "a snapshot with no running resolver is refused: {said}"
    );
    assert!(
        said.contains(&format!(
            "the resolver that published its state (pid {pid}) is not running"
        )),
        "the refusal names the resolver that is not running: {said}"
    );

    // A published resolver that accepts the request and answers nothing: the
    // state names a live process (this test) and an API bind this test holds
    // and never answers on. The test holds no clock: the command must end by
    // itself, at the refresh interval the state publishes.
    let silent = std::net::TcpListener::bind(deployment.free_bind()).unwrap();
    let bind = silent.local_addr().unwrap().to_string();
    let state_path = deployment.root.join(".stado/resolver-state.json");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert!(
        state["refresh_seconds"].is_u64(),
        "the resolver publishes the interval its readers are measured by: {state}"
    );
    state["pid"] = json!(std::process::id());
    state["api_bind"] = json!(bind);
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let abandoned = deployment.run("snapshot-silent", &["resolver", "snapshot"]);
    drop(silent);
    let said = String::from_utf8_lossy(&abandoned.stderr).into_owned();
    assert!(
        !abandoned.status.success(),
        "a snapshot whose resolver answers nothing is refused: {said}"
    );
    assert!(
        said.contains(&format!(
            "the resolver answering at {bind} accepted the request and answered nothing"
        )),
        "the refusal names the resolver API that answered nothing: {said}"
    );
}

#[test]
fn following_a_generation_ends_on_the_resolvers_own_answer() {
    let mut deployment = Deployment::start();
    deployment.cli("config-init", &["config", "init"]);
    let api_bind = deployment.free_bind();
    let generation = deployment.import_registry(&api_bind);
    let (held, above) = (
        generation.to_string(),
        generation.saturating_add(generation),
    );
    assert!(
        above > generation,
        "the template's generation is counted from above zero"
    );
    let refused = |deployment: &Deployment, name: &str, wanted: &str, said: &str| {
        let output = deployment.run(name, &["resolver", "follow", "--generation", wanted]);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            !output.status.success() && stderr.contains(said),
            "follow {wanted}: {stderr}"
        );
    };
    refused(
        &deployment,
        "follow-unpublished",
        &held,
        "no resolver has published its state on this host",
    );
    deployment.serve();
    let followed = deployment.cli(
        "follow-held",
        &["resolver", "follow", "--generation", &held],
    );
    assert!(
        followed.contains(&format!("holds directory generation {generation} or later")),
        "{followed}"
    );
    // On the authority, the generation its store holds, with no upstream to ask.
    let authority = deployment.cli("follow-authority", &["resolver", "follow"]);
    assert!(
        authority.contains(&format!("holds directory generation {generation} or later")),
        "{authority}"
    );
    let said = format!("it holds generation {generation}, so that registry does not carry directory generation {above} yet");
    refused(&deployment, "follow-above", &above.to_string(), &said);
    // A write that lands in the authority's store is what its snapshot answers next, at once.
    let (token, file) = (
        deployment.cli("token", &["registry", "pull", "--generation-only"]),
        deployment.root.join("registry.json"),
    );
    let pushed: Value = serde_json::from_str(&deployment.cli(
        "push",
        &[
            "registry",
            "push",
            &file.to_string_lossy(),
            "--if-generation",
            token.trim(),
            "--json",
        ],
    ))
    .unwrap();
    let snapshot: Value = serde_json::from_str(
        deployment
            .cli("after-push", &["resolver", "snapshot"])
            .trim(),
    )
    .unwrap();
    assert_eq!(
        snapshot["store_version"], pushed["generation"],
        "{snapshot} after {pushed}"
    );
    let pid = deployment.stop();
    refused(
        &deployment,
        "follow-stopped",
        &held,
        &format!("the resolver that published last (pid {pid}) is not running"),
    );
}

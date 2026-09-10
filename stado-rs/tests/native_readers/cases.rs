use super::*;
#[test]
#[ignore = "Probierz runs the real launchd reader lifecycle on a dedicated macOS host"]
fn convergence_reloads_a_cached_private_stado_definition_once() {
    let mut fixture = Fixture::new();
    let private_identity = file_identity(&fixture.private_binary);
    let root_identity = file_identity(&fixture.root_binary);
    assert_ne!(
        (private_identity.device, private_identity.inode),
        (root_identity.device, root_identity.inode),
        "the private and delivered Stado copies must be distinct files"
    );

    fixture.bootstrap();
    fixture.wait_until_listening(Duration::from_secs(60));
    fixture.assert_dashboard_serves_product_route();
    let private_pid = fixture.wait_for_pid(None, Duration::from_secs(30));
    assert_maps(
        &fixture,
        private_pid,
        &fixture.private_binary,
        &private_identity,
    );

    // launchd still holds the private ProgramArguments it bootstrapped, while
    // its source of truth on disk now names the delivered root.
    fixture.write_plist(&fixture.root_binary);
    assert_eq!(
        fixture.declared_program(),
        fixture.root_binary.to_string_lossy().into_owned(),
        "the on-disk plist must name the delivered root"
    );
    assert_eq!(
        fixture.pid(),
        Some(private_pid),
        "rewriting the plist alone must not replace the loaded process"
    );
    let cached = assert_maps(
        &fixture,
        private_pid,
        &fixture.private_binary,
        &private_identity,
    );
    assert_eq!(
        cached["program"],
        fixture.private_binary.to_string_lossy().into_owned(),
        "launchd must still hold the private cached program before convergence"
    );

    let first = fixture.converge();
    assert!(
        first.status.success(),
        "first convergence failed: {}",
        said(&first)
    );
    let root_pid = fixture.wait_for_pid(Some(private_pid), Duration::from_secs(60));
    fixture.wait_until_listening(Duration::from_secs(60));
    let reloaded = assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);
    assert_eq!(
        reloaded["program"],
        fixture.root_binary.to_string_lossy().into_owned(),
        "convergence did not reload the changed ProgramArguments"
    );
    fixture.assert_dashboard_serves_product_route();

    let second = fixture.converge();
    assert!(
        second.status.success(),
        "repeated convergence failed: {}",
        said(&second)
    );
    assert_eq!(
        fixture.wait_for_pid(None, Duration::from_secs(30)),
        root_pid,
        "a process already mapping the delivered root was restarted again"
    );
    let repeated = assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);
    assert_eq!(
        repeated["program"],
        fixture.root_binary.to_string_lossy().into_owned(),
        "repeated convergence changed the loaded definition"
    );
    fixture
        .cleanup()
        .unwrap_or_else(|error| panic!("native-reader cleanup was not proven: {error}"));
}

#[test]
#[ignore = "Probierz runs the real launchd reader lifecycle on a dedicated macOS host"]
fn service_update_reloads_a_cached_global_stado_definition_once() {
    let mut fixture = Fixture::new();
    fixture.write_plist(&fixture.root_binary);
    fixture.bootstrap();
    fixture.wait_until_listening(Duration::from_secs(60));
    let root_pid = fixture.wait_for_pid(None, Duration::from_secs(30));
    let root_identity = file_identity(&fixture.root_binary);
    assert_maps(&fixture, root_pid, &fixture.root_binary, &root_identity);

    let first = fixture.update_private_reader();
    assert!(
        first.status.success(),
        "private service update failed: {}",
        said(&first)
    );
    let private_path = fixture
        .home
        .join(".stado/services")
        .join(&fixture.label)
        .join("current/darwin-arm/stado");
    assert_eq!(
        fixture.declared_program(),
        private_path.to_string_lossy(),
        "service update did not move the declaration to its installed private tree"
    );
    let private_image = fs::canonicalize(&private_path).expect("installed private Stado exists");
    let private_identity = file_identity(&private_image);
    assert_eq!(private_identity.sha256, root_identity.sha256);
    let private_pid = fixture.wait_for_pid(Some(root_pid), Duration::from_secs(60));
    fixture.wait_until_listening(Duration::from_secs(60));
    assert_maps(&fixture, private_pid, &private_image, &private_identity);
    fixture.assert_dashboard_serves_product_route();

    let second = fixture.update_private_reader();
    assert!(
        second.status.success(),
        "repeated private update failed: {}",
        said(&second)
    );
    assert_eq!(
        fixture.wait_for_pid(None, Duration::from_secs(30)),
        private_pid,
        "replaying the same archive restarted an already-current private reader"
    );
    assert_maps(&fixture, private_pid, &private_image, &private_identity);

    let current = private_path.parent().unwrap().parent().unwrap();
    let previous_link = fs::read_link(current).expect("private current link");
    let previous_plist = fs::read(&fixture.plist).expect("installed private declaration");
    let wrong_layout = fixture.home.join("wrong-layout.tar.gz");
    write_stado_archive(&wrong_layout, &fixture.root_binary, "bin/stado");
    let refused = fixture
        .command(&[
            "service",
            "update",
            &fixture.label,
            "--host",
            HOST,
            "--from-archive",
            wrong_layout.to_str().expect("wrong archive path"),
            "--refresh-image",
            "--json",
        ])
        .output()
        .expect("real archive refusal command runs");
    println!("archive refusal: {}", said(&refused));
    assert!(
        !refused.status.success(),
        "an incompatible layout was accepted"
    );
    assert_eq!(fs::read_link(current).unwrap(), previous_link);
    assert_eq!(fs::read(&fixture.plist).unwrap(), previous_plist);
    assert_eq!(
        fixture.wait_for_pid(None, Duration::from_secs(30)),
        private_pid
    );
    assert_maps(&fixture, private_pid, &private_image, &private_identity);
    fixture
        .cleanup()
        .unwrap_or_else(|error| panic!("private-reader cleanup was not proven: {error}"));
}

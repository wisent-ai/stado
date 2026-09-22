//! What a declared cleaner reports, including when it reaches nothing and when
//! it runs out of the seconds it was given.

use std::fs;

use crate::fixture::Fixture;

/// nothing was broken and nothing was missing, so nothing said anything.
#[test]
fn a_cleaner_that_reaches_no_build_output_says_so_and_names_the_root() {
    let fixture = Fixture::new();
    let first = fixture.home.join("code/alpha/target");
    let second = fixture.home.join("code/beta/target");
    for tree in [&first, &second] {
        fs::create_dir_all(tree).expect("create the build tree");
        fs::write(
            tree.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .expect("write cache tag");
        fs::write(tree.join("artifact.bin"), vec![0_u8; 256 * 1024]).expect("write build output");
    }

    let output = fixture.report("120", &["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let block = &document["coverage"]["build_output"];

    assert_eq!(
        block["measured"].as_bool(),
        Some(true),
        "an unread census must never read as a measured one: {block}"
    );
    assert_eq!(
        block["unreached_trees"].as_i64(),
        Some(2),
        "both trees should be outside every declared root: {block}"
    );
    assert_eq!(
        block["suggested_root"].as_str(),
        fixture.home.join("code").to_str(),
        "the block does not name the root that would reach them: {block}"
    );
    let remedy = block["remedy"].as_str().unwrap_or_default();
    assert!(
        remedy.starts_with("stado space cleaners declare ")
            && remedy.ends_with(&format!(
                "--cleaner build_caches --root {}",
                fixture.home.join("code").display()
            )),
        "the remedy is not a command an operator can run: {remedy}"
    );

    let text = fixture.report("120", &[]);
    let printed = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(
        printed.contains("build output:")
            && printed.contains("outside every declared cleaner root"),
        "the text report does not carry the finding: {printed}"
    );
    fixture.cleanup();
}

/// The build-cache verdict is a second walk with its own budget, and until
/// 2026-09-20 exceeding it failed the whole command: on lukasz-macbook
/// `space report` printed the free space, the watermarks, the janitor's last
/// pass and every coverage row, then exited 1 with
/// `the build-cache verdict ... did not finish within 120s`. The attribution
/// walk beside it has reported its own overrun instead of dying since it was
/// given a budget; this makes the two agree.
#[test]
fn a_verdict_that_runs_out_of_seconds_is_reported_and_the_report_still_stands() {
    let fixture = Fixture::new();
    let cache = fixture.home.join("work/target");
    fs::create_dir_all(&cache).expect("create the tagged tree");
    fs::write(
        cache.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write cache tag");

    let output = fixture
        .command("0", &[])
        // Small enough that the walk cannot finish, and fractional because
        // the budget accepts fractions exactly so a case can prove the bound.
        .env("STADO_CACHE_VERDICT_BUDGET_SECONDS", "0.001")
        .output()
        .expect("run stado space report");
    fixture.retain(&[], &output);

    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(0),
        "a slow verdict failed the whole report; stderr: {stderr}"
    );
    assert!(
        stderr.contains("build cache verdict incomplete"),
        "the overrun was not reported: {stderr}"
    );
    assert!(
        stderr.contains("did not finish within"),
        "the report did not say what ran out: {stderr}"
    );
    assert!(
        text.contains("free:"),
        "the figures the report had already read were thrown away: {text}"
    );
    fixture.cleanup();
}

/// The compressor and the lifetime swapouts are read on every pass, and
/// until 2026-09-19 they were printed nowhere: charless-mac-mini reported
/// 4487 MiB available and swap 71%, both inside their watermarks, while its
/// compressor held 3.5 GiB and the released Brama was quarantined twice in
/// one hour for a readiness probe it could not answer. The text now carries

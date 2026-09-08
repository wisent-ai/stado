//! `service endpoint-check`: does the env file agree with what is listening?
//!
//! The "live endpoint" here is a `TcpListener` this test binds on loopback,
//! and the process the product names as holding it is this test process — its
//! own pid, read back out of the table. The dead endpoint is a port this test
//! bound and released. Nothing about the socket table is simulated: the
//! product reads this machine's, with the same fixed `lsof` flags `host exec`
//! already allows.

use std::net::TcpListener;

use crate::{assert_head_matches_disk, effective_on_disk, on_disk, stderr, stdout, Fleet};

/// The row for one key in the KEY LINE DECLARED PORT LISTENING PROCESS table.
fn endpoint_row<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find(|line| line.split_whitespace().next() == Some(key))
        .unwrap_or_else(|| panic!("no endpoint row for {key} in:\n{text}"))
}

#[test]
fn endpoint_check_names_the_process_holding_a_live_loopback_port() {
    let fleet = Fleet::new();
    // A real socket, held by this test process for the duration of the check.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let live = listener.local_addr().unwrap().port();
    let path = fleet.env_file(&format!(
        "WC_SKARBIEC_URL=http://127.0.0.1:{live}\nWELES_QUEUE=default\n"
    ));

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        out.status.success(),
        "endpoint-check failed on a live endpoint: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);
    let line = endpoint_row(&text, "WC_SKARBIEC_URL");
    assert!(
        line.contains("listening"),
        "a held port is not reported as listening:\n{text}"
    );
    // The endpoint judged is the one the file on disk declares, and the
    // process named as holding it is this test process.
    assert!(
        line.contains(&effective_on_disk(&path, "WC_SKARBIEC_URL")),
        "the row does not carry the file's own declaration:\n{text}"
    );
    assert!(
        line.contains(&format!("(pid {})", std::process::id())),
        "the holding process is not this test's pid {}:\n{text}",
        std::process::id()
    );
    // A value that is not an endpoint is not invented into one.
    assert!(
        !text.contains("WELES_QUEUE"),
        "a non-endpoint value was reconciled:\n{text}"
    );
    drop(listener);
}

#[test]
fn endpoint_check_fails_when_a_declared_loopback_endpoint_is_dead() {
    let fleet = Fleet::new();
    // A port this test held and released: nothing is listening there now, and
    // the kernel will not have handed it out again inside one test.
    let dead = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let path = fleet.env_file(&format!("WC_SKARBIEC_URL=http://127.0.0.1:{dead}\n"));
    let before = on_disk(&path);

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        !out.status.success(),
        "endpoint-check passed a dead dependency:\n{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("nothing is listening where WC_SKARBIEC_URL points"),
        "got: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    assert!(
        endpoint_row(&text, "WC_SKARBIEC_URL").contains("dead"),
        "the dead endpoint is not in the table:\n{text}"
    );
    // A failing check diagnoses the file; it does not edit it.
    assert_eq!(
        on_disk(&path),
        before,
        "endpoint-check rewrote the file it judged"
    );
}

#[test]
fn endpoint_check_judges_the_effective_assignment_not_the_shadowed_one() {
    let fleet = Fleet::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let live = listener.local_addr().unwrap().port();
    let dead = {
        let scratch = TcpListener::bind("127.0.0.1:0").unwrap();
        scratch.local_addr().unwrap().port()
    };
    // The live endpoint first, the dead one second: a reader that took the
    // first assignment, or that collapsed the file into a map by insertion
    // order, would report this file as healthy.
    let path = fleet.env_file(&format!(
        "WC_SKARBIEC_URL=http://127.0.0.1:{live}\nexport WC_SKARBIEC_URL=http://127.0.0.1:{dead}\n"
    ));

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        !out.status.success(),
        "endpoint-check judged the shadowed assignment:\n{}",
        stdout(&out)
    );
    let text = stdout(&out);
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("WC_SKARBIEC_URL"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "a shadowed assignment was reconciled too:\n{text}"
    );
    // The one row judged carries the declaration the file's LAST assignment
    // holds, which is the dead one.
    assert!(
        rows[0].contains(&effective_on_disk(&path, "WC_SKARBIEC_URL")) && rows[0].contains("dead"),
        "the effective assignment is not the one judged:\n{text}"
    );
    assert!(
        text.contains("duplicates: WC_SKARBIEC_URL"),
        "the shadowing that caused this is not reported:\n{text}"
    );
    drop(listener);
}

#[test]
fn endpoint_check_does_not_judge_a_remote_endpoint_against_this_host() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_UPSTREAM_URL=https://api.example.com/v1\n");

    let out = fleet.endpoint_check(path.to_str().unwrap(), &[]);
    assert!(
        out.status.success(),
        "a remote endpoint was judged against this host's sockets: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);
    let line = endpoint_row(&text, "WELES_UPSTREAM_URL");
    assert!(line.contains("remote"), "got:\n{text}");
    assert!(
        line.contains(&effective_on_disk(&path, "WELES_UPSTREAM_URL")),
        "the row does not carry the file's own declaration:\n{text}"
    );
}

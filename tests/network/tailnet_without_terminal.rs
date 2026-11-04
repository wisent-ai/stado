//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test network-tailnet-without-terminal -- --nocapture`.
//! Stado reads this Mac's real Tailscale client the way a launchd unit runs
//! it: no TERM, no terminal on any standard stream.
//!
//! Run with no terminal, the program inside the macOS Tailscale app takes
//! itself for the app being opened and starts its window: it printed "The
//! Tailscale GUI failed to start" instead of the JSON Stado asked for, or,
//! with the app's window up, never answered, and a `stado serve` waiting on it
//! at start never bound its API or resolver while launchd called the unit
//! running. The journey re-runs itself with an empty environment and no
//! terminal, reads the client through `stado::remote::tailnet::own`, and
//! requires the map it states. On a machine with no Tailscale client the
//! journey says so and checks nothing, since there is nothing to ask.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "STADO_TAILNET_WITHOUT_TERMINAL_CHILD";
/// Longer than any answer the client gives; a client that took its window
/// instead never answers at all.
const ANSWERED_WITHIN: Duration = Duration::from_secs(60);

#[test]
fn the_tailscale_client_answers_a_process_with_no_terminal() {
    if std::env::var_os(CHILD).is_some() {
        match stado::remote::tailnet::own() {
            Ok(Some((members, own))) => {
                println!("members={members} own={}", own.as_deref().unwrap_or("running"));
                assert!(members > 0, "the client answered a map without one address");
            }
            Ok(None) => println!("no-client"),
            Err(cause) => panic!("the Tailscale client was not read without a terminal: {cause}"),
        }
        return;
    }
    let home = std::env::var_os("HOME").expect("HOME names the account the client serves");
    let mut child = Command::new(std::env::current_exe().expect("this test's own executable"))
        .args(["--exact", "the_tailscale_client_answers_a_process_with_no_terminal", "--nocapture"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("re-run this journey without a terminal");
    let started = Instant::now();
    while child.try_wait().expect("the run without a terminal can be observed").is_none() {
        if started.elapsed() > ANSWERED_WITHIN {
            let _ = child.kill();
            let said = child.wait_with_output().expect("collect the run that did not answer");
            panic!(
                "the Tailscale client did not answer a process with no terminal within {ANSWERED_WITHIN:?}: {}",
                String::from_utf8_lossy(&said.stderr)
            );
        }
        std::thread::park_timeout(Duration::from_millis(200));
    }
    let said = child.wait_with_output().expect("collect the run without a terminal");
    let stdout = String::from_utf8_lossy(&said.stdout).into_owned();
    println!("{stdout}");
    assert!(
        said.status.success(),
        "the run without a terminal failed: {stdout}{}",
        String::from_utf8_lossy(&said.stderr)
    );
    assert!(
        stdout.contains("members=") || stdout.contains("no-client"),
        "the run without a terminal said neither a map nor that no client is installed: {stdout}"
    );
}

//! A page nobody received must not exit zero.
//!
//! # The invariant
//!
//! `stado alerts send` is the fleet's way of reaching the operator. Its
//! per-channel outcome went to stderr as an `[alert]` line and its exit
//! status was zero whether the mail left the machine or every provider
//! refused it. A person tailing the monitor could see the failure; a program
//! could not, and the first program to depend on it — Weles, opening an
//! operator request while it waits for a phone approval it cannot give
//! itself — recorded "the operator was told" from a status that could not
//! say otherwise.
//!
//! # What is defended here
//!
//! The command's answer, not its logging: with the enabled channel set naming
//! something that resolves to no adapter, the command fails, says nobody was
//! paged, and names the channels it was told to use — so the caller learns
//! the page did not happen at the moment it did not happen.

use std::process::Command;

/// The executable cargo just built for this test run.
const STADO: &str = env!("CARGO_BIN_EXE_stado");
/// An enabled channel name no adapter answers to, so resolution is empty
/// without touching this deployment's real Resend key or phone number.
const UNRESOLVABLE_CHANNEL: &str = "channel-no-adapter-answers";

#[test]
fn a_send_that_reaches_no_channel_fails_and_says_nobody_was_paged() {
    let output = Command::new(STADO)
        .args([
            "alerts",
            "send",
            "Weles is waiting for one action from you.",
            "--subject",
            "operator request",
        ])
        .env("STADO_ALERT_CHANNELS", UNRESOLVABLE_CHANNEL)
        .output()
        .unwrap_or_else(|error| panic!("{STADO} alerts send did not run: {error}"));

    assert!(
        !output.status.success(),
        "a send that paged nobody exited {:?}; stdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nobody was paged"),
        "the refusal must say nobody was paged; got: {stderr}"
    );
    assert!(
        stderr.contains(UNRESOLVABLE_CHANNEL),
        "the refusal must name the enabled channels; got: {stderr}"
    );
}

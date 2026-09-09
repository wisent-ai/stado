//! Real current-host journey for the fixed retained-Tailscale-log read.
//!
//! The journey runs the built Stado CLI against an isolated local registry
//! naming the machine executing the test. The production host channel
//! therefore takes its current-host path and executes the operating system's
//! real logging tool: `/usr/bin/log` on macOS or `/usr/bin/journalctl` on Linux.
//! No executable is substituted and no SSH destination exists in the fixture.
//! The same story then sends the fixed read through a real Stado dashboard
//! without mutation confirmation and verifies that an exact provider sign-in
//! remains behind `RUN_MUTATION`; its deliberately unknown target makes even a
//! confirmation-classification regression stop before any provider contact.
//!
//! It runs by default. The retained logging service it needs is this host's
//! own — `/usr/bin/log` on a macOS arm64 machine, `/usr/bin/journalctl` on a
//! Linux amd64 one — so the dependency is present wherever the suite runs, and
//! a missing logging executable or a failed native read fails this journey
//! instead of being skipped or replaced.
//!
//! Process stdout, stderr, arguments, and exit status are retained under the
//! repository's ignored `.wisent-output/host-exec-retained-logs` directory.
//! Empty native output is recorded as empty; it is not treated as proof about
//! Funnel.
//!
//! The files are split so each stays inside the three hundred line limit this
//! repository enforces on itself:
//!
//! - [`story`]: the platform's declared argv and the widenings to refuse.
//! - [`journey`]: the isolated store, the invocation, the retained evidence.
//! - [`dashboard`]: the same read through the product's own HTTP surface.
//! - [`verdicts`]: what each receipt must say.

mod dashboard;
mod journey;
mod story;
mod verdicts;

use serde_json::json;

use crate::journey::{write_private, Journey};
use crate::verdicts::{assert_native_read, assert_refused};

#[tokio::test]
async fn retained_tailscale_logs_are_a_fixed_native_read_and_cannot_be_widened() {
    let journey = Journey::new();

    // Execute the whole story before judging it so every real process result is
    // retained even if one later assertion identifies a regression.
    let retained = journey.invoke("retained-log-read", journey.story.words);
    let wider_time = journey.invoke("refuse-wider-time", journey.story.wider_time);
    let wider_source = journey.invoke("refuse-extra-process-or-unit", journey.story.wider_source);
    let modifying = journey.invoke("refuse-modifying-log-operation", journey.story.modifying);
    journey.verify_dashboard_boundary().await;

    let (stdout_bytes, stderr_bytes) = assert_native_read(&journey, &retained);
    let canonical = journey.story.words.join(" ");
    assert_refused(&wider_time, journey.story.wider_time, &canonical);
    assert_refused(&wider_source, journey.story.wider_source, &canonical);
    assert_refused(&modifying, journey.story.modifying, &canonical);
    journey.assert_unchanged();

    write_private(
        &journey.root.join("journey.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema": "stado.host-exec-retained-log-journey.v1",
            "status": "completed",
            "test_source_revision": env!("STADO_SOURCE_REVISION"),
            "host": journey.hostname,
            "platform": journey.story.platform,
            "native_program": journey.story.program,
            "native_exit_code": retained.status.code(),
            "native_stdout_bytes": stdout_bytes,
            "native_stderr_bytes": stderr_bytes,
            "native_output_empty": stdout_bytes == 0 && stderr_bytes == 0,
            "refusals": 3,
            "config_unchanged": true,
            "registry_unchanged": true,
            "dashboard_read_without_confirmation": true,
            "dashboard_provider_sign_in_without_confirmation": "refused",
            "funnel_verdict": "not asserted",
        }))
        .unwrap(),
    );
    println!(
        "retained-log read verified: platform={}; native_exit=0; stdout_bytes={stdout_bytes}; stderr_bytes={stderr_bytes}; refusals=3; dashboard_read_only=true; dashboard_sign_in_refused=true; funnel=not-asserted; evidence={}",
        journey.story.platform,
        journey.root.display(),
    );
}

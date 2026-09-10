//! Real current-host journey for the fixed retained-Tailscale-log read.
//!
//! The ignored journey runs the built Stado CLI against an isolated local
//! registry naming the machine executing the test. The production host channel
//! therefore takes its current-host path and executes the operating system's
//! real logging tool: `/usr/bin/log` on macOS or `/usr/bin/journalctl` on Linux.
//! No executable is substituted and no SSH destination exists in the fixture.
//! The same story then sends the fixed read through a real Stado dashboard
//! without mutation confirmation and verifies that an exact provider sign-in
//! remains behind `RUN_MUTATION`; its deliberately unknown target makes even a
//! confirmation-classification regression stop before any provider contact.
//!
//! Process stdout, stderr, arguments, and exit status are retained under the
//! repository's ignored `.wisent-output/host-exec-retained-logs` directory.
//! Empty native output is recorded as empty; it is not treated as proof about
//! Funnel. A missing logging executable or a failed native read fails this
//! journey instead of being skipped or replaced.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::timeout;

use serde_json::{json, Value};

mod dashboard;
mod journey;
mod story;

use dashboard::*;
use journey::*;
use story::*;

#[tokio::test]
#[ignore = "requires and records the current host's real retained logging service"]
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

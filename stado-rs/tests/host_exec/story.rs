//! The exact native reads this journey expects, per platform: the words the
//! refusals carry, the argv the host runs, and the widenings it must refuse.
use super::*;

pub(crate) const TARGET: &str = "retained-log-current-host";
pub(crate) const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub(crate) const PROVIDER_SIGN_IN: &[&str] = &[
    "start-with-skarbiec",
    "subscription",
    "sign-in",
    "codex",
    "--login-item",
    "codex-wisent-google-sso",
    "--reason",
    "codex-grant-disowned-2026-08-27-gateway-has-one-live-provider",
    "--login-timeout-ms",
    "900000",
    "--json",
];

pub(crate) const MACOS_LOG_WORDS: &[&str] = &[
    "log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
pub(crate) const MACOS_LOG_ARGV: &[&str] = &[
    "/usr/bin/log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
pub(crate) const MACOS_WIDER_TIME: &[&str] = &[
    "log",
    "show",
    "--last",
    "2h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];
pub(crate) const MACOS_EXTRA_PROCESS: &[&str] = &[
    "log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
    "--process",
    "stado-retained-log-probierz-never-runs",
];
// `log config` is the modifying verb. The deliberately invalid mode keeps the
// journey harmless even if a future regression accidentally executes it.
pub(crate) const MACOS_MODIFYING_LOG: &[&str] =
    &["log", "config", "--mode", "definitely-not-a-log-mode"];

pub(crate) const LINUX_LOG_WORDS: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
];
pub(crate) const LINUX_LOG_ARGV: &[&str] = &[
    "/usr/bin/journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
];
pub(crate) const LINUX_WIDER_TIME: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-2h",
    "--no-pager",
    "--output",
    "short-iso",
];
pub(crate) const LINUX_EXTRA_UNIT: &[&str] = &[
    "journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
    "--unit",
    "stado-retained-log-probierz-never-runs.service",
];
// Vacuuming is a modifying journal operation. Its invalid duration ensures the
// native tool could not vacuum anything even if this refusal ever regressed.
pub(crate) const LINUX_MODIFYING_LOG: &[&str] =
    &["journalctl", "--vacuum-time", "definitely-not-a-duration"];

pub(crate) struct NativeStory {
    pub(crate) platform: &'static str,
    pub(crate) program: &'static str,
    pub(crate) words: &'static [&'static str],
    pub(crate) argv: &'static [&'static str],
    pub(crate) wider_time: &'static [&'static str],
    pub(crate) wider_source: &'static [&'static str],
    pub(crate) modifying: &'static [&'static str],
}

pub(crate) fn native_story() -> NativeStory {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => NativeStory {
            platform: "darwin-arm64",
            program: "/usr/bin/log",
            words: MACOS_LOG_WORDS,
            argv: MACOS_LOG_ARGV,
            wider_time: MACOS_WIDER_TIME,
            wider_source: MACOS_EXTRA_PROCESS,
            modifying: MACOS_MODIFYING_LOG,
        },
        ("linux", "x86_64") => NativeStory {
            platform: "linux-amd64",
            program: "/usr/bin/journalctl",
            words: LINUX_LOG_WORDS,
            argv: LINUX_LOG_ARGV,
            wider_time: LINUX_WIDER_TIME,
            wider_source: LINUX_EXTRA_UNIT,
            modifying: LINUX_MODIFYING_LOG,
        },
        (os, arch) => panic!(
            "blocked: retained-log host-exec journey requires macOS arm64 or Linux amd64, got {os}-{arch}"
        ),
    }
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| panic!("create retained evidence {}: {error}", path.display()));
    file.write_all(bytes)
        .unwrap_or_else(|error| panic!("write retained evidence {}: {error}", path.display()));
}

pub(crate) fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(
        output.status.success(),
        "blocked: the real hostname executable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let hostname = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_string();
    assert!(
        !hostname.is_empty(),
        "the real current host has no hostname"
    );
    hostname
}

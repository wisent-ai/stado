//! The exact words this platform's retained-log read is made of, and the
//! three widenings that must be refused: a wider time window, an extra
//! process or unit, and an operation that would modify the log itself.

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
pub(crate) const MACOS_MODIFYING_LOG: &[&str] = &["log", "config", "--mode", "definitely-not-a-log-mode"];

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
pub(crate) const LINUX_MODIFYING_LOG: &[&str] = &["journalctl", "--vacuum-time", "definitely-not-a-duration"];

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


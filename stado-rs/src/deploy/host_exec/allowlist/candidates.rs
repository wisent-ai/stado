//! Which absolute paths one program is installed at, and the lookup every
//! other reader in this fleet resolves that program through.

use super::programs::{
    ANDROID_DEBUG_BRIDGE, APPIUM_CLI, CADDY_PROXY, CARGO_CLI, CUA_DRIVER, GIT_CLI, NODE_RUNTIME,
    NPM_CLI, TAILSCALE_PROGRAM, TMUX_CLI, UV_INSTALLER,
};

/// Every absolute path a program in this table is installed at, for the
/// programs whose location differs per platform.
///
/// The order is the one every other reader in this repository already probes
/// (`scripts/diagnose-tailscale-serve-host.sh`,
/// `scripts/reconcile-stado-object-tailnet-route-host.sh`, the enrolment
/// script in `cli/fleet/invite.rs`), so `host exec` cannot disagree with them
/// about which binary is the tailscale CLI on a given host.
///
/// A program absent from this table has exactly one path — its `argv[0]` — and
/// keeps the plain [`crate::deploy::host_channel::run_program`] transport.
pub const PROGRAM_CANDIDATES: &[(&str, &[&str])] = &[
    (
        TAILSCALE_PROGRAM,
        &[
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
            "/usr/local/bin/tailscale",
            "/opt/homebrew/bin/tailscale",
            TAILSCALE_PROGRAM,
        ],
    ),
    // The order Weles's kimi login trajectory probes, so this read and that
    // install cannot disagree about whether the host has uv.
    (UV_INSTALLER, &[UV_INSTALLER, "/usr/local/bin/uv"]),
    // The four programs a Spis crawl placement needs, each at the paths this
    // fleet actually installs it at, home-relative first. A single-path entry
    // would report "no such file" for a host that has the program one prefix
    // over, and the first probe run of these entries on 2026-09-03 proved that
    // the system prefixes alone answer "missing" for a program that is present:
    // rustup writes cargo into `~/.cargo/bin`, and
    // `~/.stado/bin/install-cua-driver` links its CLI into `~/.local/bin` off
    // the bundle it dittos into `/Applications/CuaDriver.app`.
    (
        APPIUM_CLI,
        &[
            "~/.npm-global/bin/appium",
            "~/.local/bin/appium",
            APPIUM_CLI,
            "/usr/local/bin/appium",
        ],
    ),
    (
        ANDROID_DEBUG_BRIDGE,
        &[
            "~/Library/Android/sdk/platform-tools/adb",
            ANDROID_DEBUG_BRIDGE,
            "/usr/local/bin/adb",
        ],
    ),
    (
        CUA_DRIVER,
        &[
            "~/.local/bin/cua-driver",
            "/Applications/CuaDriver.app/Contents/MacOS/cua-driver",
            CUA_DRIVER,
            "/usr/local/bin/cua-driver",
        ],
    ),
    (
        CARGO_CLI,
        &[
            "~/.cargo/bin/cargo",
            "/Users/Shared/.cargo/bin/cargo",
            CARGO_CLI,
            "/usr/local/bin/cargo",
        ],
    ),
    (
        TMUX_CLI,
        &[TMUX_CLI, "/usr/local/bin/tmux", "/usr/bin/tmux"],
    ),
    // Git, at real installations only. Homebrew first, then the Command Line
    // Tools' own git INSIDE the developer directory, then a Linux path.
    //
    // `/usr/bin/git` is absent on purpose and must stay absent: on macOS that
    // path is the `xcode-select` shim, and on a host with no Command Line
    // Tools it opens the installer WINDOW rather than printing a version, so
    // probing it could raise a consent dialog on an unattended host. The CLT
    // path below is the real binary the shim would have forwarded to, and it
    // simply does not exist when the tools are absent — which is the honest
    // answer this probe wants.
    (
        GIT_CLI,
        &[
            GIT_CLI,
            "/usr/local/bin/git",
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
        ],
    ),
    // The order every Node reader in this repository already probes — the
    // launcher script in `deploy::weles_browser_runtime`, and the host reads in
    // `cli/host.rs` and `cli/seed_freshness.rs` — so a `host exec` answer about
    // a host's Node cannot name a different binary from the one a managed unit
    // executes on that same host.
    (
        NODE_RUNTIME,
        &[NODE_RUNTIME, "/usr/local/bin/node", "/usr/bin/node"],
    ),
    (NPM_CLI, &[NPM_CLI, "/usr/local/bin/npm", "/usr/bin/npm"]),
    (
        CADDY_PROXY,
        &[CADDY_PROXY, "/usr/local/bin/caddy", "/usr/bin/caddy"],
    ),
];

/// Every absolute path this fleet installs one program at, in probe order, or
/// `None` for a program whose only path is its own `argv[0]`.
///
/// Exposed because a second reader appeared and copying the list into it would
/// have created exactly the disagreement [`PROGRAM_CANDIDATES`] exists to
/// prevent: `deploy::mobile_runtime` verifies and installs the mobile runtime
/// and has to resolve `appium` and `adb` the same way this allowlist's probe
/// does, or `stado host exec TARGET -- appium --version` and
/// `stado host mobile-runtime TARGET` could name different binaries on one
/// machine and disagree about whether the host is ready. One table, two
/// readers.
///
/// It is also the answer to "which path should a placement use": the fleet's
/// hosts do not carry these directories on a non-interactive `PATH`, so a
/// consumer resolves a declared absolute path from here and never searches the
/// environment.
pub fn program_candidates(program: &str) -> Option<&'static [&'static str]> {
    PROGRAM_CANDIDATES
        .iter()
        .find(|(name, _)| *name == program)
        .map(|(_, candidates)| *candidates)
}
/// Cargo's candidates in the one order every host reader must use.
pub fn cargo_candidates() -> &'static [&'static str] {
    program_candidates(CARGO_CLI).expect("cargo is in the program candidate table")
}

#[cfg(test)]
mod tests {
    use super::super::programs::GIT_PROGRAM;
    use super::*;

    /// `/usr/bin/git` is the `xcode-select` shim: on a host with no Command
    /// Line Tools, running it opens the installer WINDOW instead of printing
    /// a version. A read-only allowlist must not be able to raise a consent
    /// dialog on an unattended host, so the shim stays out of the candidates
    /// and this test is what stops it being helpfully added back.
    #[test]
    fn the_git_probe_never_reaches_the_xcode_select_shim() {
        let candidates = program_candidates(GIT_PROGRAM).expect("git is in the table");
        assert!(
            !candidates.contains(&"/usr/bin/git"),
            "the /usr/bin/git shim must never be probed: {candidates:?}"
        );
        assert!(
            candidates.contains(&"/Library/Developer/CommandLineTools/usr/bin/git"),
            "the real Command Line Tools git must be probed instead"
        );
        assert!(candidates.iter().all(|path| path.starts_with('/')));
    }
}

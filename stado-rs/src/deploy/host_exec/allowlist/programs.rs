//! Every program this table may run, at the canonical spelling the rest of
//! the fleet names it by.

/// The tailscale CLI, as the fleet's Linux hosts install it.
///
/// It is `argv[0]` of the two tailscale entries, so [`super::ApprovedCommand::display`]
/// spells them `tailscale …` — the name of the program, in the case every
/// operator and every script in this repository types it. On macOS the same CLI
/// ships inside the application bundle instead, which is why the entry needs
/// [`super::PROGRAM_CANDIDATES`]: one program, two install layouts, one spelling.
pub const TAILSCALE_PROGRAM: &str = "/usr/bin/tailscale";

/// The Appium server CLI's canonical name in [`super::PROGRAM_CANDIDATES`].
pub const APPIUM_PROGRAM: &str = APPIUM_CLI;

/// The Android platform-tools bridge's canonical name in
/// [`super::PROGRAM_CANDIDATES`].
pub const ADB_PROGRAM: &str = ANDROID_DEBUG_BRIDGE;

/// The Node runtime's canonical name in [`super::PROGRAM_CANDIDATES`].
///
/// Needed by any reader that runs a Node shim rather than a compiled binary:
/// `appium` starts `#!/usr/bin/env node`, and a non-interactive ssh session on
/// a Homebrew host carries none of Node's directories on `PATH`, so the shim
/// answers `env: node: No such file or directory` and reads as broken while
/// being perfectly installed.
/// [`crate::deploy::host_exec::channel::candidate_script`] makes the same
/// argument one level up, and puts every candidate's directory on `PATH` for
/// that reason.
pub const NODE_PROGRAM: &str = NODE_RUNTIME;

/// Git's canonical name in [`super::PROGRAM_CANDIDATES`].
///
/// Exposed for the same reason [`APPIUM_PROGRAM`] is: a second reader must
/// resolve it from this table, not from a list of its own — and in git's case
/// a hand-written list is how `/usr/bin/git` gets added back.
pub const GIT_PROGRAM: &str = GIT_CLI;

/// The tmux multiplexer, at the paths this fleet's hosts install it at.
///
/// Spis's terminal families drive the product under test inside a tmux
/// session, so this is their precondition in the same way cargo is every
/// worker's. Unapproved, `stado host exec TARGET -- tmux -V` answers "not an
/// approved host-exec command" and every CLI and TUI preflight refuses every
/// host — a refusal that reads as "this machine has no tmux" and is really a
/// question the channel was never allowed to ask.
pub const TMUX_CLI: &str = "/opt/homebrew/bin/tmux";

/// tmux's canonical name in [`super::PROGRAM_CANDIDATES`].
pub const TMUX_PROGRAM: &str = TMUX_CLI;

/// The Kimi Code CLI. Its installed help defines the supported login flags;
/// these entries inspect that interface without starting a login.
pub const KIMI_CLI: &str = "~/.kimi-code/bin/kimi";

/// The registry-managed Stado binary on either supported host platform.
pub const STADO_CLI: &str = "~/.stado/bin/stado";

/// The release-managed Skarbiec binary on a fleet host, where every other
/// reader in this repository addresses it (`host_capability`, the service
/// grant scripts, the release wall).
pub const SKARBIEC_CLI: &str = "~/.stado/bin/skarbiec";

/// The uv package installer, at the two absolute paths every reader in this
/// fleet probes for it — including Weles's kimi login trajectory, whose pinned
/// CLI install depends on one of them existing.
pub const UV_INSTALLER: &str = "/opt/homebrew/bin/uv";

/// The Appium server CLI, as Homebrew and a global npm prefix lay it down.
///
/// Spis's crawl coordinator asks this host, through this very channel, whether
/// the mobile placement can run at all before it submits a job. An answer of
/// "not an approved host-exec command" reads as a policy gap and hides the
/// only fact that matters: whether the program is on the machine.
pub const APPIUM_CLI: &str = "/opt/homebrew/bin/appium";

/// The Android platform-tools bridge, at the two absolute paths the fleet's
/// macOS hosts install it at.
///
/// `which adb` below answers a different question — whether the login shell's
/// PATH carries it — and answers `not found` on a host that has the binary
/// outside a non-interactive ssh PATH, which is exactly the case on Homebrew
/// installs.
pub const ANDROID_DEBUG_BRIDGE: &str = "/opt/homebrew/bin/adb";

/// The Cua Driver CLI, which drives native macOS and desktop applications.
///
/// `cua-driver doctor --json` is its own read-only self-check and the exact
/// prerequisite Spis's desktop placement probes; `~/.stado/bin/install-cua-driver`
/// is what puts it on a host, and this entry is how an operator learns whether
/// that ever ran here.
pub const CUA_DRIVER: &str = "/opt/homebrew/bin/cua-driver";

/// Cargo, at the paths this fleet installs Rust at: the shared toolchain the
/// always-on hosts keep outside any one login's home first, then Homebrew,
/// then a local rustup prefix.
///
/// Every Spis crawl worker runs as `cargo run --release` at a pinned revision
/// on the placement host, so "does this host have cargo, and where" is the
/// precondition of every native, terminal and command-line family.
pub const CARGO_CLI: &str = "/opt/homebrew/bin/cargo";

/// Git, at the paths a real installation puts it, and DELIBERATELY NOT at
/// `/usr/bin/git`.
///
/// `/usr/bin/git` on macOS is not git. It is Apple's `xcode-select` shim, and
/// on a host without the Command Line Tools installed, running it OPENS THE
/// CLT INSTALLER WINDOW instead of answering. So the obvious probe — ask
/// `/usr/bin/git --version`, the path every script reaches for — is the one
/// spelling that can pop a consent dialog on an unattended fleet host, which
/// is the opposite of what a read-only allowlist is for. The shim is excluded
/// from the candidates below for exactly that reason, and this paragraph is
/// the reason written down beside the entry, because it is the kind of thing
/// that gets "simplified" back in by the next person who notices `/usr/bin`
/// is missing from a list of git paths.
///
/// Spis's terminal (TUI) worker runs `git` to build its fixture repository,
/// so "does this host have a real git, and where" is that family's
/// precondition; it is asked here so a host can be refused before it claims
/// a slot, and the answer is the absolute path the worker's command is then
/// built from.
pub const GIT_CLI: &str = "/opt/homebrew/bin/git";
/// The Node runtime, at the absolute paths this fleet installs it at.
///
/// A non-interactive ssh login reads no shell profile, and the fleet's Node
/// comes from Homebrew on the macOS hosts and from the distribution's own
/// package on the Linux one, so `node` is on nobody's PATH over this channel
/// and the question has to be asked of the paths directly. It is `argv[0]` of
/// the node entry, so [`super::ApprovedCommand::display`] spells it `node --version`
/// — the name of the program, in the case every operator types it.
pub const NODE_RUNTIME: &str = "/opt/homebrew/bin/node";

/// The npm CLI, at the absolute paths it is installed beside that Node at.
///
/// A separate program from the runtime and therefore a separate question: an
/// install can leave one behind without the other, and `npm ci` is what a web
/// product's release actually runs.
pub const NPM_CLI: &str = "/opt/homebrew/bin/npm";

/// The Caddy reverse proxy, at the absolute paths a host may carry it at.
///
/// The public web edge terminates TLS for a product hostname with a
/// registry-managed Caddy unit, and the unit's program is this binary, so this
/// is the path the unit will name and the path the read must probe.
pub const CADDY_PROXY: &str = "/opt/homebrew/bin/caddy";

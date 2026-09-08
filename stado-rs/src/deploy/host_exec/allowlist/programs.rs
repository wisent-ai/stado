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
/// worker's. It went unapproved, which meant `stado host exec TARGET --
/// tmux -V` answered "not an approved host-exec command" and every CLI and
/// TUI preflight refused every host — a refusal that reads as "this machine
/// has no tmux" and is really a question the channel was never allowed to
/// ask. Found on 2026-09-03 by running the terminal preflight against
/// lukasz-macbook, which does carry tmux.
pub const TMUX_CLI: &str = "/opt/homebrew/bin/tmux";

/// tmux's canonical name in [`super::PROGRAM_CANDIDATES`].
pub const TMUX_PROGRAM: &str = TMUX_CLI;

/// Brama's own service launcher, as the fleet installs it in the managed
/// account's home.
///
/// It is `argv[0]` of the sign-in entries, and it is the canonical spelling
/// rather than a path that exists: the launcher ships inside the release
/// bundle, so where it actually lives is
/// [`crate::deploy::host_exec::channel::AccountProgram::candidates`].
/// Running the gateway binary directly would be the wrong program: `brama
/// subscription sign-in` needs the admission credential the launcher acquires
/// from Skarbiec under Brama's own workload identity at every start, and the
/// launcher runs a named CLI verb inside exactly that environment. Nothing
/// here carries a secret — the launcher fetches it on the host and it never
/// reaches an argument vector.
pub const BRAMA_LAUNCHER: &str = "~/.stado/bin/start-with-skarbiec";

/// The Kimi Code CLI, as the fleet's macOS hosts install it.
///
/// A Weles trajectory drives this program, and a trajectory that passes it a
/// flag it does not accept fails with the CLI's own one-line refusal and
/// nothing else — which is how `kimi login --json` cost the fleet its kimi
/// subscription renewals without anybody being able to say what the CLI does
/// accept. Its own help is the answer, and reading it from here is how that
/// question gets settled against the installed version rather than against a
/// pinned one in a script.
pub const KIMI_CLI: &str = "~/.kimi-code/bin/kimi";

/// The registry-managed Stado binary on either supported host platform.
pub const STADO_CLI: &str = "~/.stado/bin/stado";

/// The uv package installer, at the two absolute paths every reader in this
/// fleet probes for it — including Weles's kimi login trajectory, whose pinned
/// CLI install depends on one of them existing.
pub const UV_INSTALLER: &str = "/opt/homebrew/bin/uv";

/// The Appium server CLI, as Homebrew and a global npm prefix lay it down.
///
/// Spis's crawl coordinator asks this host, through this very channel, whether
/// the mobile placement can run at all before it submits a job. Until
/// 2026-09-03 the answer it got was "not an approved host-exec command", which
/// reads as a policy gap and hid the only fact that mattered: whether the
/// program is on the machine.
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

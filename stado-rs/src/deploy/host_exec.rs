//! `stado host exec TARGET -- CMD…` — run one APPROVED read-only command
//! on a registry host through the shared ssh channel.
//!
//! NO Python original: item six of `docs/missing-commands.md`, whose
//! wording is the whole design — "with an allowlist, not free shell".
//!
//! This is not a remote shell and must never become one. `stado host exec`
//! exists so an operator diagnosing a wedged box does not have to keep a
//! private ssh alias outside the registry-authorized channel; it does not
//! exist to run arbitrary code as the login user of every machine in the
//! fleet.
//!
//! Three independent barriers stand between the operator's words and the
//! host, in this order:
//!
//! 1. **Character rejection.** Every word the operator typed must consist
//!    only of characters no shell treats specially ([`is_shell_safe`]).
//!    Anything carrying `;`, `|`, `&`, `$`, backtick, quote, newline,
//!    redirection or glob is refused by name before anything else happens.
//! 2. **Exact allowlist match.** The words, joined, must equal one entry of
//!    [`APPROVED_COMMANDS`] exactly. There is no prefix match, no
//!    pass-through of extra arguments, and no operator-supplied path — a
//!    command that took a path would be a command that could read
//!    `~/.ssh/id_ed25519`.
//! 3. **Fixed argv.** What actually runs is the matched entry's own
//!    `argv`, a compile-time constant of absolute paths. The operator's
//!    words select an entry; they never become part of the command line.
//!    [`crate::deploy::host_channel::ssh_program_argv`] then shell-quotes
//!    each fixed word for the remote login shell.
//!
//! Barrier 3 alone makes injection impossible, which is precisely why
//! barriers 1 and 2 are worth having: they mean the guarantee does not
//! depend on the table being perfectly curated, and they give the operator
//! a real error instead of a silent mismatch.
//!
//! Almost every entry is read-only. The exceptions are the sign-in entries at
//! the end of the table, which exist because a provider grant the vendor has
//! disowned is repaired by one command and no read can substitute for it; each
//! states in its own [`ApprovedCommand::why`] exactly what it changes. Every
//! entry, read or repair, still takes no operator-supplied argument and
//! carries its own justification.
//!
//! An entry whose program the managed account owns rather than the system —
//! anything under `~` — is described once more in [`ACCOUNT_PROGRAMS`], which
//! supplies the fixed environment and the time budget that program needs. Those
//! words are compile-time constants of this module too, so barrier three is
//! unchanged: `$HOME` expands on the far side and nothing the operator typed
//! reaches the host except the choice of entry.
//!
//! An entry whose fixed path ARGUMENTS name something inside that account's
//! home — rather than its program — is listed once more in
//! [`HOME_ROOTED_READS`], which runs it from that home. Those paths are
//! written relative for a reason barrier one imposes: `~` is a character a
//! shell acts on, so an argument spelled `~/…` would be refused as the
//! operator's own word and the entry could never be selected at all.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::host_channel;
use super::{py_str_repr, shlex_quote, DeployError, Runner};

const RESOLVED_EXECUTABLE_MARKER: &str = "STADO_RESOLVED_EXECUTABLE\t";

/// The exact document `stado host exec --json` prints.
///
/// Typed, and `deny_unknown_fields`, because this is a machine document: one
/// of our own reconcile scripts reads it with jq, so a misspelled or renamed
/// key has to be a compile error here rather than a quietly different report
/// its consumer discovers in production. `schema` is what a consumer gates a
/// version on, and a map carrying only the target and its connection detail
/// gives it nothing to gate on.
///
/// Built directly instead of by validating a
/// [`host_channel::base_report`] map on the way out: that map's string keys
/// would be a second shape for the same document, checked no earlier than
/// the call that happens to exercise it, whereas one struct cannot drift
/// from itself. Field names, and the omission of the conditional ones,
/// reproduce the map this replaced, so the printed document is unchanged.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostExecReceipt {
    schema: String,
    target: String,
    ssh: Option<String>,
    ssh_fallbacks: Vec<crate::targets::SshConnectionPath>,
    command: String,
    argv: Vec<String>,
    /// Only for an entry installed at more than one path: `argv[0]` is then
    /// one candidate among several and is not evidence of where the host
    /// found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    program_candidates: Option<Vec<String>>,
    /// Only when the host reported which candidate it execed. The account
    /// script resolves `$program` in the remote shell and reports nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_executable: Option<String>,
    /// Only for an account-owned entry, which runs under its own budget.
    /// Without it a channel cut at the cap reads like a program that failed
    /// fast, and the two ask for different next steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u64>,
    stdout: String,
    stderr: String,
    exit_code: i32,
    status: String,
    /// Only on failure: the remote's own last line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `status` for a command that ran and exited clean.
pub const OK_STATUS: &str = "ok";

/// A host-exec failure that states its own [`crate::failure::FailureCode`]
/// where it is created, instead of leaving one to be guessed from its prose.
///
/// On 2026-09-03 `host exec charless-mac-mini -- ls -la …` was refused by the
/// allowlist and reported `error_code=timeout`, `retryable=true`. Nothing had
/// timed out. The refusal was built as a bare [`DeployError`], flattened to a
/// string by the CLI, and the code was then reconstructed by
/// [`crate::failure::classify_message`], whose `timeout` needle is the bare
/// substring `"timeout"` — and this refusal prints the whole allowlist, three
/// entries of which carry `--login-timeout-ms`. **The refusal matched its own
/// help text**, so every unapproved command on every host told its caller to
/// retry something that can never succeed.
///
/// Narrowing the needle would have left that design in place and handed the
/// next help-text collision to the next reader. So the code travels with the
/// failure: `code: Some(_)` is knowledge from the construction site and is
/// used verbatim, while `None` marks a failure that genuinely arrived as text
/// and keeps `classify_message` as its last resort.
///
/// `help` is the second half of the repair. The allowlist stays in front of
/// the operator, but out of `message`, so the classified and logged sentence
/// is the refusal itself — short enough to survive the log line's detail
/// bound whole, and with no vocabulary in it but its own.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExecRefusal {
    /// What this failure knows itself to be, when it knows.
    pub code: Option<crate::failure::FailureCode>,
    /// The operator sentence: what was refused, and why.
    pub message: String,
    /// Operator help that is not part of the failure — the approved
    /// spellings — printed beside it and never classified.
    pub help: Option<String>,
}

impl ExecRefusal {
    /// A refusal this module states outright: the words are understood, and
    /// the allowlist does not admit them.
    ///
    /// [`crate::failure::FailureCode::Refused`] — "an explicit policy refused
    /// this command" — is the whole of what happened. Nothing is missing, no
    /// credential was presented, nothing is down, and waiting changes
    /// nothing: only the words or the table can change. It is not retryable,
    /// and its exit code is the one the caller already chose.
    ///
    /// The code was added to `wisent-errors` for this call site rather than
    /// picked from the seven that were there. `not_found` reads as a missing
    /// path and would have sent an operator to check paths and permissions
    /// until they disbelieved the error, which is the cost the `timeout`
    /// misclassification was already imposing, only quieter.
    fn unapproved(message: String) -> Self {
        Self {
            code: Some(crate::failure::FailureCode::Refused),
            message,
            help: Some(format!("approved commands: {}", allowlist())),
        }
    }
}

impl From<DeployError> for ExecRefusal {
    /// Everything else this module reaches — the registry, the channel, the
    /// host — still arrives as prose, and prose is what `classify_message`
    /// exists for.
    fn from(error: DeployError) -> Self {
        Self {
            code: None,
            message: error.0,
            help: None,
        }
    }
}

/// The punctuation an operator's word may contain on top of ASCII
/// alphanumerics. Every one of these is inert to `/bin/sh`: no expansion,
/// no word splitting, no redirection, no globbing.
const SAFE_PUNCTUATION: &str = "-_./:%";

/// One approved remote program.
#[derive(Debug)]
pub struct ApprovedCommand {
    /// What actually runs: an absolute program path followed by its FIXED
    /// arguments. Nothing the operator types is ever appended to it.
    pub argv: &'static [&'static str],
    /// Why running this unattended, as the registry-managed login user, is
    /// safe. An entry without a defensible answer here does not belong in
    /// the table.
    pub why: &'static str,
}

impl ApprovedCommand {
    /// The spelling an operator types: the program's basename followed by
    /// its fixed arguments. Derived from [`Self::argv`] so the table has
    /// exactly one source of truth.
    pub fn display(&self) -> String {
        let mut words: Vec<&str> = Vec::new();
        if let Some((program, arguments)) = self.argv.split_first() {
            words.push(program.rsplit('/').next().unwrap_or(program));
            words.extend(arguments.iter().copied());
        }
        words.join(" ")
    }

    /// Every absolute path this entry's program may be installed at, in probe
    /// order. A one-element slice — `argv[0]` itself — for every program that
    /// lives in exactly one place.
    pub fn candidates(&self) -> &'static [&'static str] {
        let Some((program, _)) = self.argv.split_first() else {
            return &[];
        };
        PROGRAM_CANDIDATES
            .iter()
            .find(|(named, _)| named == program)
            .map_or(std::slice::from_ref(program), |(_, paths)| paths)
    }
}

/// The tailscale CLI, as the fleet's Linux hosts install it.
///
/// It is `argv[0]` of the two tailscale entries, so [`ApprovedCommand::display`]
/// spells them `tailscale …` — the name of the program, in the case every
/// operator and every script in this repository types it. On macOS the same CLI
/// ships inside the application bundle instead, which is why the entry needs
/// [`PROGRAM_CANDIDATES`]: one program, two install layouts, one spelling.
const TAILSCALE_PROGRAM: &str = "/usr/bin/tailscale";

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
/// keeps the plain [`host_channel::run_program`] transport.
const PROGRAM_CANDIDATES: &[(&str, &[&str])] = &[
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

/// The Appium server CLI's canonical name in [`PROGRAM_CANDIDATES`].
pub const APPIUM_PROGRAM: &str = APPIUM_CLI;

/// The Android platform-tools bridge's canonical name in
/// [`PROGRAM_CANDIDATES`].
pub const ADB_PROGRAM: &str = ANDROID_DEBUG_BRIDGE;

/// The Node runtime's canonical name in [`PROGRAM_CANDIDATES`].
///
/// Needed by any reader that runs a Node shim rather than a compiled binary:
/// `appium` starts `#!/usr/bin/env node`, and a non-interactive ssh session on
/// a Homebrew host carries none of Node's directories on `PATH`, so the shim
/// answers `env: node: No such file or directory` and reads as broken while
/// being perfectly installed. [`candidate_script`] makes the same argument one
/// level up, and puts every candidate's directory on `PATH` for that reason.
pub const NODE_PROGRAM: &str = NODE_RUNTIME;

/// Git's canonical name in [`PROGRAM_CANDIDATES`].
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
const TMUX_CLI: &str = "/opt/homebrew/bin/tmux";

/// tmux's canonical name in [`PROGRAM_CANDIDATES`].
pub const TMUX_PROGRAM: &str = TMUX_CLI;

/// Brama's own service launcher, as the fleet installs it in the managed
/// account's home.
///
/// It is `argv[0]` of the sign-in entries, and it is the canonical spelling
/// rather than a path that exists: the launcher ships inside the release
/// bundle, so where it actually lives is [`AccountProgram::candidates`].
/// Running the gateway binary directly would be the wrong program: `brama
/// subscription sign-in` needs the admission credential the launcher acquires
/// from Skarbiec under Brama's own workload identity at every start, and the
/// launcher runs a named CLI verb inside exactly that environment. Nothing
/// here carries a secret — the launcher fetches it on the host and it never
/// reaches an argument vector.
const BRAMA_LAUNCHER: &str = "~/.stado/bin/start-with-skarbiec";

/// The Kimi Code CLI, as the fleet's macOS hosts install it.
///
/// A Weles trajectory drives this program, and a trajectory that passes it a
/// flag it does not accept fails with the CLI's own one-line refusal and
/// nothing else — which is how `kimi login --json` cost the fleet its kimi
/// subscription renewals without anybody being able to say what the CLI does
/// accept. Its own help is the answer, and reading it from here is how that
/// question gets settled against the installed version rather than against a
/// pinned one in a script.
const KIMI_CLI: &str = "~/.kimi-code/bin/kimi";

/// The uv package installer, at the two absolute paths every reader in this
/// fleet probes for it — including Weles's kimi login trajectory, whose pinned
/// CLI install depends on one of them existing.
const UV_INSTALLER: &str = "/opt/homebrew/bin/uv";

/// The Appium server CLI, as Homebrew and a global npm prefix lay it down.
///
/// Spis's crawl coordinator asks this host, through this very channel, whether
/// the mobile placement can run at all before it submits a job. Until
/// 2026-09-03 the answer it got was "not an approved host-exec command", which
/// reads as a policy gap and hid the only fact that mattered: whether the
/// program is on the machine.
const APPIUM_CLI: &str = "/opt/homebrew/bin/appium";

/// The Android platform-tools bridge, at the two absolute paths the fleet's
/// macOS hosts install it at.
///
/// `which adb` below answers a different question — whether the login shell's
/// PATH carries it — and answers `not found` on a host that has the binary
/// outside a non-interactive ssh PATH, which is exactly the case on Homebrew
/// installs.
const ANDROID_DEBUG_BRIDGE: &str = "/opt/homebrew/bin/adb";

/// The Cua Driver CLI, which drives native macOS and desktop applications.
///
/// `cua-driver doctor --json` is its own read-only self-check and the exact
/// prerequisite Spis's desktop placement probes; `~/.stado/bin/install-cua-driver`
/// is what puts it on a host, and this entry is how an operator learns whether
/// that ever ran here.
const CUA_DRIVER: &str = "/opt/homebrew/bin/cua-driver";

/// Cargo, at the paths this fleet installs Rust at: the shared toolchain the
/// always-on hosts keep outside any one login's home first, then Homebrew,
/// then a local rustup prefix.
///
/// Every Spis crawl worker runs as `cargo run --release` at a pinned revision
/// on the placement host, so "does this host have cargo, and where" is the
/// precondition of every native, terminal and command-line family.
const CARGO_CLI: &str = "/opt/homebrew/bin/cargo";

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
const GIT_CLI: &str = "/opt/homebrew/bin/git";
/// The Node runtime, at the absolute paths this fleet installs it at.
///
/// A non-interactive ssh login reads no shell profile, and the fleet's Node
/// comes from Homebrew on the macOS hosts and from the distribution's own
/// package on the Linux one, so `node` is on nobody's PATH over this channel
/// and the question has to be asked of the paths directly. It is `argv[0]` of
/// the node entry, so [`ApprovedCommand::display`] spells it `node --version`
/// — the name of the program, in the case every operator types it.
const NODE_RUNTIME: &str = "/opt/homebrew/bin/node";

/// The npm CLI, at the absolute paths it is installed beside that Node at.
///
/// A separate program from the runtime and therefore a separate question: an
/// install can leave one behind without the other, and `npm ci` is what a web
/// product's release actually runs.
const NPM_CLI: &str = "/opt/homebrew/bin/npm";

/// The Caddy reverse proxy, at the absolute paths a host may carry it at.
///
/// The public web edge terminates TLS for a product hostname with a
/// registry-managed Caddy unit, and the unit's program is this binary, so this
/// is the path the unit will name and the path the read must probe.
const CADDY_PROXY: &str = "/opt/homebrew/bin/caddy";

/// The prefix that marks a program, or one of its environment values, as
/// living under the login user's home rather than at a system path.
const HOME_RELATIVE: &str = "~/";

/// The managed service directory `com.wisent.weles-admission` runs out of,
/// relative to the managed account's home.
///
/// Written once, so the three entries that read it cannot drift apart about
/// which directory they are describing. `stado service release` installs
/// every release for a managed service under `.stado/services/<name>/` in a
/// directory named for the archive digest and points `current` at it, so this
/// prefix plus a digest is the whole of that service's installed history.
const WELES_ADMISSION_SERVICE_DIR: &str = ".stado/services/weles-admission";

/// Which installed release directory the admission unit executes through.
const WELES_ADMISSION_CURRENT: &[&str] = &[
    "/usr/bin/readlink",
    ".stado/services/weles-admission/current",
];

/// Every installed release directory for that service, with `current`'s own
/// target rendered beside it.
const WELES_ADMISSION_VERSIONS: &[&str] = &["/bin/ls", "-l", WELES_ADMISSION_SERVICE_DIR];

/// The compiled worker modules in the runtime tree that service's launcher
/// resolves — the directory `weles-api-server.mjs` imports `dispatch.js` from.
const WELES_ADMISSION_WORKER_MODULES: &[&str] = &[
    "/bin/ls",
    ".stado/services/weles-admission/current/darwin-arm/runtime/dist/worker",
];

/// What the launcher itself sees when it decides whether to unpack: the
/// payload archive, the derived `runtime` tree, and their timestamps.
const WELES_ADMISSION_RELEASE_TREE: &[&str] = &[
    "/bin/ls",
    "-l",
    ".stado/services/weles-admission/current/darwin-arm",
];
/// Size and modification epoch of the log the long-running manual Figma
/// export redirects away from Stado's canonical job log.
const FIGMA_EXPORT_LOG_STAT: &[&str] = &[
    "/usr/bin/stat",
    "-f",
    "%z:%m",
    ".stado/work/figma-export/export.log",
];

/// Total allocated KiB below the manual Figma export's fixed work tree.
const FIGMA_EXPORT_WORK_TREE_SIZE: &[&str] = &["/usr/bin/du", "-sk", ".stado/work/figma-export"];

/// Every entry whose fixed path arguments name something inside the managed
/// account's home rather than a system path.
///
/// Keyed on the entry's whole `argv`, the way [`PROGRAM_CANDIDATES`] and
/// [`ACCOUNT_PROGRAMS`] are keyed on a program: one more table beside the
/// allowlist rather than one more field on every entry in it.
///
/// Their paths are written RELATIVE and resolved by an explicit `cd "$HOME"`
/// in [`home_rooted_script`], instead of being spelled `~/…`. Barrier one of
/// this module refuses any operator word carrying a character a shell would
/// act on, and `~` is one, so a `~/…` argument would make its own entry
/// unreachable through [`approve`] — the operator could never type the
/// spelling that selects it. The remote login shell already starts in the
/// managed account's home, so standing in it changes nothing about where
/// these reads land; it only stops the entry from depending on that.
const HOME_ROOTED_READS: &[&[&str]] = &[
    WELES_ADMISSION_CURRENT,
    WELES_ADMISSION_VERSIONS,
    WELES_ADMISSION_RELEASE_TREE,
    WELES_ADMISSION_WORKER_MODULES,
    FIGMA_EXPORT_LOG_STAT,
    FIGMA_EXPORT_WORK_TREE_SIZE,
];

/// Is this entry's fixed argv one of the home-rooted reads?
fn home_rooted(argv: &[&str]) -> bool {
    HOME_ROOTED_READS.contains(&argv)
}

/// The remote script for a read inside the managed account's home: stand in
/// that home, then become the entry's own fixed argv.
///
/// Every word is a compile-time constant of this module and is quoted for the
/// remote shell anyway. The operator's words selected the entry and reach the
/// host in nothing else, so barrier three holds exactly as it does on the
/// [`host_channel::run_program`] path.
fn home_rooted_script(argv: &[&str]) -> String {
    let fixed = argv
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    format!("set -eu\ncd \"$HOME\"\nexec {fixed}\n")
}

/// What an entry whose program the managed account owns needs on top of its
/// fixed argv.
#[derive(Debug)]
struct AccountProgram {
    /// `argv[0]` of every entry this describes, exactly as the entry spells
    /// it. Keyed on the program, like [`PROGRAM_CANDIDATES`], so one row
    /// covers every verb of the same program.
    program: &'static str,
    /// Every path in the account's home this program is installed at, in probe
    /// order. The first executable one runs.
    candidates: &'static [&'static str],
    /// Fixed environment for that program, home-relative where a value is a
    /// path. Compile-time constants of this module: an operator's words select
    /// an entry and never become part of this.
    environment: &'static [(&'static str, &'static str)],
    /// The wall-clock budget for the whole run.
    ///
    /// [`host_channel::remote_timeout`] is two minutes, which is right for a
    /// read and wrong for a repair that walks a real single-sign-on and a
    /// consent screen in a browser on the far side. Cutting the channel
    /// mid-flight would leave the operator unable to tell a refused sign-in
    /// from one still running.
    timeout_seconds: u64,
}

/// Every program in the table that the managed account owns.
const ACCOUNT_PROGRAMS: &[AccountProgram] = &[
    AccountProgram {
        program: BRAMA_LAUNCHER,
        // The launcher is part of the release bundle, and the live bundle is the
        // `current` link the service unit itself runs through -- never a pinned
        // version, which would go stale at the next release and send a repair into
        // a launcher older than the vault it talks to. Both platform directory
        // spellings the fleet has shipped are probed, newest layout first, and the
        // standalone copy some accounts keep in `~/.stado/bin` is last.
        candidates: &[
            "~/.stado/services/brama/current/darwin-arm64/bin/start-with-skarbiec",
            "~/.stado/services/brama/current/darwin-arm/bin/start-with-skarbiec",
            BRAMA_LAUNCHER,
        ],
        // Two paths this run must not share with the gateway it is repairing.
        //
        // The launcher ends whatever holds its capability-broker socket and then
        // rebinds it. On its stable default path that is the live gateway's own
        // broker, so a CLI run beside a serving Brama would take the service's
        // credential redemption down with it. And the runtime directory is named
        // after the installation, which for a CLI run out of the live bundle is
        // the live one: the launcher rebuilds the subscription manifest and the
        // capability catalog in it at every start, so sharing it would rewrite the
        // serving gateway's own runtime state. Both get a copy of their own under
        // the fleet's scratch area, and the service is untouched.
        environment: &[
            (
                "BRAMA_CAP_SOCKET",
                "~/.stado/work/brama-sign-in/capability.sock",
            ),
            ("BRAMA_RUNTIME_DIR", "~/.stado/work/brama-sign-in/runtime"),
        ],
        timeout_seconds: 1500,
    },
    AccountProgram {
        program: KIMI_CLI,
        // The three places this fleet's hosts have it, in the order the Weles
        // trajectory's own resolver probes them, so `host exec` and the
        // trajectory cannot disagree about which binary is the Kimi CLI.
        candidates: &["~/.local/bin/kimi", KIMI_CLI, "/opt/homebrew/bin/kimi"],
        // Nothing. Its help is a read; giving it an environment would be
        // giving it a home and a session it has no business reading here.
        environment: &[],
        timeout_seconds: 60,
    },
];

/// The account-owned program behind an entry, if this is one.
fn account_program(program: &str) -> Option<&'static AccountProgram> {
    ACCOUNT_PROGRAMS
        .iter()
        .find(|account| account.program == program)
}

/// A home-relative word as the remote shell should read it: its own `$HOME`
/// followed by the quoted remainder. A word that is already absolute is just
/// quoted.
fn home_anchored(word: &str) -> String {
    match word.strip_prefix(HOME_RELATIVE) {
        Some(rest) => format!("\"$HOME\"/{}", shlex_quote(rest)),
        None => shlex_quote(word),
    }
}

/// The remote script for an account-owned entry: find the installed copy,
/// export the entry's fixed environment, then become it.
///
/// Every word is a compile-time constant of this module and is quoted for the
/// remote shell; the only thing that expands on the host is its own `$HOME`.
/// The operator's words selected the entry and reach the host in nothing else,
/// so barrier three holds exactly as it does on the
/// [`host_channel::run_program`] path.
fn account_script(account: &AccountProgram, arguments: &[&str]) -> String {
    let mut script = String::from("set -eu\nprogram=\n");
    for candidate in account.candidates {
        script.push_str(&format!(
            "[ -n \"$program\" ] || [ ! -x {candidate} ] || program={candidate}\n",
            candidate = home_anchored(candidate)
        ));
    }
    script.push_str(&format!(
        "[ -n \"$program\" ] || {{ printf '%s\\n' {} >&2; exit 127; }}\n",
        shlex_quote(&format!(
            "this program is installed at none of its approved paths in the managed \
             account's home on this host: {}",
            account.candidates.join(", ")
        ))
    ));
    for (name, value) in account.environment {
        script.push_str(&format!("{name}={}\nexport {name}\n", home_anchored(value)));
    }
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    script.push_str(&format!("exec \"$program\" {fixed}\n"));
    script
}

/// The allowlist.
///
/// Declared as a slice, not an array, so adding an entry never means
/// touching a length. Ordered roughly by how often an operator reaches for
/// it while a box is misbehaving.
pub const APPROVED_COMMANDS: &[ApprovedCommand] = &[
    ApprovedCommand {
        argv: &["/usr/bin/uptime"],
        why: "reads kernel uptime and load counters; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/df", "-h"],
        why: "reads mounted-filesystem statistics; -h is a fixed display unit, and with no \
              path argument it cannot be pointed at anything",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/"],
        why: "attributes a full root filesystem two directory levels deep; -x stays on one \
              filesystem, -k is a fixed unit, the depth and the root are fixed words, and du \
              writes nothing. Added 2026-08-19: the linux builder sat at 100% used and every \
              declared cleaner and reclaim stage measured zero, so the operator had no \
              sanctioned way to even name what was eating the disk",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "1", "/private/tmp"],
        why: "attributes the OS scratch directory one level deep; -x stays on one filesystem, \
              -k is a fixed unit, the depth and the path are fixed words, and du writes \
              nothing. Added 2026-09-04: charless-mac-mini reached 1.1 GB free of 239 GB, \
              which took the object API, the registry authority and every Skarbiec \
              decryption on that host down at once, and the root-level attribution named \
              /private/tmp as the second largest consumer at 14.2 GB while every declared \
              cleaner and reclaim stage measured zero. Nothing in this table could say what \
              those bytes were, so they could neither be defended nor reclaimed",
    },
    ApprovedCommand {
        argv: &["/bin/ls", "-lt", "/private/tmp"],
        why: "lists the OS scratch directory's own entries with their modification times; the \
              path is a fixed word, no operator selector is appended, and ls writes nothing. \
              Sizes alone cannot separate a wedged product's live scratch from an abandoned \
              tree, and deleting an unclassified 14 GB is not a repair. Added 2026-09-04 \
              beside the du entry above, for the same outage",
    },
    ApprovedCommand {
        argv: &["/usr/bin/who"],
        why: "reads the login-session table; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/launchctl", "list"],
        why: "lists the calling user's launchd jobs. `list` without a label is the read-only \
              verb; the mutating verbs (bootout, bootstrap, kickstart, enable) are absent from \
              this table and cannot be reached through it. This is the view the unmanaged \
              weles-api agent shows up in",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps", "ax", "-o", "pid", "-o", "ppid", "-o", "etime", "-o", "comm",
        ],
        why: "lists process identifiers, elapsed time, and executable names without command \
              arguments or environment values; it is read-only and cannot expose secret argv",
    },
    ApprovedCommand {
        argv: &["/usr/bin/top", "-l", "4", "-s", "10", "-o", "cpu"],
        why: "samples every process four times at ten-second intervals and orders the fixed \
              read-only report by CPU use. A single `ps` sample can legitimately catch an \
              I/O-bound worker at zero; this bounded thirty-second observation distinguishes \
              that moment from a process consuming no CPU throughout the interval. It takes \
              no pid, command text, file, or operator-supplied selector and writes nothing",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps", "ax", "-o", "pid", "-o", "rss", "-o", "pcpu", "-o", "comm",
        ],
        why: "reports resident memory per process, by executable name only. The two `ps` \
              entries around it show identity, parentage and elapsed time but never a byte \
              count, so the one question a thrashing host forces - which process ate the \
              memory - had no answer in this table at all. Added 2026-09-03: \
              charless-mac-mini was holding ~2.9 GB in the compressor with ~88 MB free and \
              3,277,146 swapouts, which stalled every fresh ssh session on it for 12-25 s \
              and tripped an unrelated preflight's hard timeout; `vm_stat` proved the \
              pressure was real but could not name a single owner of it. `-o rss` is a \
              kernel counter and `-o comm` is the executable's name; `-o command` - the \
              full argv, where tokens and passwords are passed - is deliberately NOT in \
              this table and cannot be reached through it. The selector is fixed to `ax` \
              and takes no pid, user, or file argument, so it cannot be pointed at \
              anything narrower or anywhere else",
    },
    ApprovedCommand {
        argv: FIGMA_EXPORT_LOG_STAT,
        why: "reads only byte size and modification epoch for the fixed manual Figma export \
              log inside the managed account's Stado work tree. Added 2026-09-04 because a \
              job can renew its lease for hours while redirecting every progress byte away \
              from the canonical zero-byte job log; without two measurements of this file \
              the fleet cannot distinguish useful work from a hang. The path and format are \
              compile-time constants, no operator word is appended, and stat writes nothing",
    },
    ApprovedCommand {
        argv: FIGMA_EXPORT_WORK_TREE_SIZE,
        why: "reads allocated KiB below only the fixed manual Figma export work tree. The \
              export writes its result below that tree while its redirected log can grow \
              independently, so measuring both at two times distinguishes output progress \
              from logging alone. The path, unit and recursion root are compile-time \
              constants, no operator word is appended, du stays within the managed account's \
              work tree, and the command writes nothing",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps",
            "axww",
            "-o",
            "pid",
            "-o",
            "ppid",
            "-o",
            "cgroup:200",
            "-o",
            "comm",
        ],
        why: "lists process identifiers, executable names, and their Linux control-group paths \
              without command arguments or environment values; it is read-only and identifies \
              the exact systemd unit behind a duplicate queue agent",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/netstat", "-anv", "-p", "tcp"],
        why: "reads the kernel TCP socket table without connecting to any endpoint; fixed \
              flags expose listeners and owning processes but accept no remote address",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/lsof", "-nP", "-iTCP", "-sTCP:LISTEN"],
        why: "names the process behind every listening TCP port. `netstat -anv -p tcp` above \
              never shows an owner, so the one question the fleet asks most often - which \
              process holds this port - was answered over ssh instead. The flags fix the \
              selection to listeners and take no argument, so it cannot be pointed at a file, \
              a user, or a remote address",
    },
    ApprovedCommand {
        argv: &["/usr/bin/crontab", "-l"],
        why: "prints the calling account's own crontab. `-l` is the read-only verb and takes \
              no argument: `-e` opens an editor, `-r` deletes the table, and neither is in \
              this table nor reachable through it; `-u <user>` would read another account's \
              and is deliberately absent. Added 2026-08-31: a process nobody could name had \
              been overwriting charless-mac-mini's janitor state file every four minutes \
              since at least that morning, with the default outcome and no writer \
              attribution, while the queue agent's own broadcast reported a healthy pass in \
              the same second. It is not a launchd job - 47 undeclared fleet labels on that \
              host, none of them a janitor - and it holds the run lock too briefly to be \
              caught by sampling, which leaves a periodic table as the only remaining place \
              it can be declared. Every reader in this group could see the file change and \
              none could name the writer",
    },
    ApprovedCommand {
        argv: &["/usr/bin/uname", "-a"],
        why: "prints kernel identification; -a only widens the fields, and there is no input \
              to interpret",
    },
    ApprovedCommand {
        argv: &["/usr/bin/sw_vers"],
        why: "prints the macOS product and build version; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/bin/vm_stat"],
        why: "prints Mach virtual-memory statistics; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/hostname", "-f"],
        why: "prints the fully qualified hostname; -f only selects the long form. Reading it \
              is how a registry `hostnames` entry gets checked against the box itself",
    },
    ApprovedCommand {
        argv: &["/usr/bin/id"],
        why: "prints the login user's uid, gid and groups; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/date", "-u"],
        why: "prints the current UTC clock; -u only selects the timezone. Clock skew is a real \
              cause of refused ssh keys and failed storage authentication, so it is worth \
              being able to read",
    },
    ApprovedCommand {
        argv: &["/usr/bin/defaults", "read", "MobileMeAccounts"],
        why: "prints which Apple accounts the login user is signed into. Some work runs only \
              on the machine that holds an identity -- a two-factor prompt appears on the \
              trusted device and nowhere else -- so `stado identity verify` must check the \
              binding rather than trust a declaration nothing re-reads. The domain is fixed, \
              `read` is the read-only verb, and the output carries account identifiers, never \
              tokens or passwords",
    },
    ApprovedCommand {
        argv: &["/usr/bin/dscl", ".", "-list", "/Users"],
        why: "lists the local account names on the host. An identity binding may name a user \
              other than the login user, and `defaults read` answers only for whoever the \
              channel logs in as, so such a binding reports unknown forever. Whether that user \
              exists at all is the question that separates a real gap from a declaration \
              nobody can ever satisfy. `.` is the local node, `-list` is the read-only verb, \
              and account names are not secrets",
    },
    ApprovedCommand {
        argv: &["/usr/bin/xcrun", "simctl", "list", "devices", "available"],
        why: "lists installed iOS Simulator runtimes and devices without booting or mutating \
              one; this is the prerequisite check for native iOS capture placement",
    },
    ApprovedCommand {
        argv: &["/usr/bin/xcrun", "devicectl", "list", "devices"],
        why: "lists Apple devices visible to CoreDevice without installing, launching, or \
              changing anything; physical-device availability decides whether App Store \
              binaries can be captured rather than simulator-only builds",
    },
    ApprovedCommand {
        argv: &["/bin/ls", "/Applications"],
        why: "lists system-wide installed application bundle names under the fixed public \
              Applications directory; it reads no user documents and changes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/bin/which", "adb"],
        why: "reports whether Android platform-tools are on the managed login's PATH; it takes \
              a fixed executable name, reads no application state, and writes nothing",
    },
    // The four crawl prerequisites, added 2026-09-03. Spis's crawl coordinator
    // preflights a placement host through this channel before it submits any
    // job, and for these four the channel answered "not an approved host-exec
    // command". That refusal is indistinguishable from "the program is
    // missing", so the 2026-09-01 crawl run recorded fifteen catalogs as
    // preflight_failed without anybody being able to say which of the two it
    // was. Each entry prints a version or a self-check, takes no
    // operator-supplied word, installs nothing and mutates nothing.
    ApprovedCommand {
        argv: &[APPIUM_CLI, "--version"],
        why: "prints the installed Appium server's version, probing the absolute paths this \
              fleet installs it at rather than the non-interactive ssh PATH -- a different \
              question that answers `not found` on a host that has the binary. It is the \
              precondition of every iOS and Android capture placement: without Appium there \
              is no driver to open an installed application with. `--version` starts no \
              server, opens no device and writes nothing",
    },
    ApprovedCommand {
        argv: &[APPIUM_CLI, "driver", "list", "--installed"],
        why: "lists which Appium drivers are actually installed, which is the half of mobile \
              readiness a version cannot answer: XCUITest for iOS and UiAutomator2 for \
              Android are separate installs, and a placement fails at the first command \
              without them. `list --installed` reads the local driver manifest; the forms \
              that change anything (`driver install`, `uninstall`, `update`) are absent from \
              this table and unreachable through it, because the allowlist matches an entry \
              exactly and never appends operator words",
    },
    ApprovedCommand {
        argv: &[ANDROID_DEBUG_BRIDGE, "version"],
        why: "prints the installed Android Debug Bridge's version from the absolute paths the \
              fleet installs platform-tools at. `which adb` above answers the PATH question \
              and returns nothing on a Homebrew install reached over ssh, which is why both \
              exist. `version` contacts no device and starts no server beyond adb's own \
              local one",
    },
    ApprovedCommand {
        argv: &[ANDROID_DEBUG_BRIDGE, "devices", "-l"],
        why: "lists the Android devices and emulators this host can currently see, with their \
              transport and model. It is the placement question for the Android family: a \
              host with adb and no device cannot capture anything. The listing names devices, \
              not their contents, and changes nothing on them",
    },
    ApprovedCommand {
        argv: &[CUA_DRIVER, "doctor", "--json"],
        why: "runs the Cua Driver's own read-only self-check and prints it as JSON: whether \
              the driver is installed and whether this host's accessibility and \
              screen-recording grants are in place. That is the exact precondition of the \
              macOS and desktop capture families, and `~/.stado/bin/install-cua-driver` is \
              what would repair it. `doctor` opens no application and grants nothing itself",
    },
    ApprovedCommand {
        argv: &[CARGO_CLI, "--version"],
        why: "prints the installed Rust toolchain's cargo version from the paths this fleet \
              installs Rust at, shared-toolchain prefix first. Every Spis crawl worker runs \
              as `cargo run --release` at a pinned revision on the placement host, so this \
              one fact decides whether the terminal, command-line, documentation and native \
              families can execute there at all. `--version` compiles nothing, fetches \
              nothing and writes nothing",
    },
    ApprovedCommand {
        argv: &[GIT_CLI, "--version"],
        why: "prints git's version from the paths a real installation puts it at, and \
              deliberately NOT from `/usr/bin/git`, which on macOS is the `xcode-select` \
              shim: on a host without the Command Line Tools that path opens the installer \
              WINDOW instead of answering, so the obvious probe is the one spelling that \
              could raise a consent dialog on an unattended fleet host. Spis's terminal \
              family builds its fixture repository with git, so this decides whether that \
              family can run at all, and the resolved path is what the worker's command is \
              then built from rather than a bare name a non-login shell cannot find. \
              `--version` reads no repository, touches no working tree and writes nothing",
    },
    ApprovedCommand {
        argv: &[TMUX_CLI, "-V"],
        why: "prints the tmux version from the absolute paths this fleet installs it at. Spis's \
              command-line and terminal families drive the product under test inside a tmux \
              session, so this is their precondition exactly as cargo is every worker's -- and \
              until this entry existed the question could not be asked at all: `tmux -V` came \
              back `not an approved host-exec command`, so both families refused every host and \
              the refusal read as a missing program. `-V` starts no server, attaches to no \
              session and writes nothing",
    },
    ApprovedCommand {
        argv: &[UV_INSTALLER, "--version"],
        why: "prints the uv package installer's version, probing the two absolute paths Weles's \
              kimi login trajectory itself probes rather than the login shell's PATH -- which \
              is a different question and answers `not found` on a host that has the binary. \
              Added 2026-09-02: that trajectory now resolves a pinned Kimi CLI version and \
              installs it through uv when the host carries a different one, and \
              charless-mac-mini carries a different one, so whether that repair can complete \
              there is entirely this one fact. `--version` takes no argument and installs \
              nothing",
    },
    ApprovedCommand {
        argv: &[
            "/usr/sbin/sysctl",
            "-n",
            "kern.maxproc",
            "kern.maxprocperuid",
        ],
        why: "reads two named kernel tunables — the system-wide and per-uid process ceilings — \
              and nothing else. `-n` prints values without names, the two keys are compile-time \
              constants rather than an operator-supplied name, and neither carries secret data. \
              This is the pair that says whether a host refusing to fork is out of process slots \
              or actually wedged; without it `host inventory` reporting probe_failed has no \
              follow-up question",
    },
    ApprovedCommand {
        argv: &["/bin/ps", "ax", "-o", "user", "-o", "pid", "-o", "comm"],
        why: "lists which login user owns each process, by executable name only. `-o comm` is \
              the executable's name; `-o command` — the full argv, where tokens and passwords \
              are passed — is deliberately NOT in this table and cannot be reached through it, \
              because the allowlist matches an entry exactly and never appends operator words. \
              Answers 'whose process is holding that port', which the pid-only listing cannot",
    },
    // The four reads a connectivity gap needs, added 2026-08-19. Between
    // 18:29 and 18:35 UTC control-host answered no ping and no ssh, then
    // came back on a direct path; every fact about that gap — when the host
    // slept and woke, whether its path was direct or relayed and to which
    // endpoint, whether its own view of the tailnet was degraded, and which
    // interface had dropped — was read by an operator over a private ssh
    // session, eleven times, because no sanctioned path existed. These are
    // that path.
    ApprovedCommand {
        argv: &["/usr/bin/pmset", "-g", "log"],
        why: "prints the power-management event log: every sleep, every wake, and the reason \
              the kernel recorded for each. `-g` is pmset's read-only getter and `log` names \
              the log to print; the verbs that change anything (`sleep`, `displaysleepnow`, \
              `schedule`, `repeat`, and the `-a`/`-b`/`-c` setting forms) are absent from this \
              table and cannot be reached through it, because the allowlist matches an entry \
              exactly and never appends operator words. A host that went quiet because it slept \
              is indistinguishable from one that crashed until this log is read",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "status", "--json"],
        why: "prints this node's own view of the tailnet as JSON: for every peer, whether the \
              current path is direct or through a relay, and which endpoint carries it. \
              `status` is the read-only verb and `--json` changes only the rendering; the verbs \
              that change anything (`up`, `down`, `set`, `login`, `logout`, `serve`, `funnel`) \
              are absent from this table. The output carries node names, tailnet addresses and \
              endpoints — the same addresses the registry already holds — and no keys beyond \
              the public ones every node publishes. This is where `direct 10.0.0.253:41641` \
              comes from, the line that said the 2026-08-19 gap had ended",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "netcheck"],
        why: "reports what this host can reach of the relay mesh: UDP reachability, whether a \
              router will map a port, and the latency to each relay region. It sends probe \
              traffic to Tailscale's own relays and writes nothing on the host and nothing to \
              the tailnet, so it is safe to run against a live machine. It answers the question \
              a one-sided ping cannot — whether the degraded path is this host's or the peer's",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "serve", "status", "--json"],
        why: "prints this node's serve and funnel handler table as JSON: which HTTPS port \
              forwards to which loopback origin, and whether funnel is on. `status` is the \
              read-only verb; the forms that change anything (`serve --bg`, `funnel`, `reset`) \
              are absent from this table because the allowlist matches an entry exactly. This \
              is the read that says a published endpoint 404s because its rule was lost, which \
              on 2026-08-24 left Jeden reading bare 404s from Brama while every beacon said \
              active, and could only be diagnosed from outside the host",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "funnel", "status"],
        why: "prints whether funnel is enabled and which origins it publishes. `status` is \
              the read-only verb and takes no operand, so nothing here can publish, retract, \
              or alter a rule. It is the postcondition half of the serve-status read: brama's \
              funnel publisher (brama/scripts/brama-funnel-publisher.sh) verifies itself with \
              exactly this command, so an operator can re-check its verdict by hand",
    },
    ApprovedCommand {
        argv: &["/sbin/ifconfig", "-a"],
        why: "lists every network interface with its addresses and flags. `-a` only widens the \
              selection to interfaces that are not up, which is the whole point: the interface \
              that dropped is the one that is no longer listed by default. There is no \
              interface operand and no address, and every configuring form of ifconfig requires \
              one, so this entry cannot change an address, a route, or an interface's state",
    },
    // The Linux half of the interface read, added 2026-09-02.
    // `stado host exec ubuntu-server-rtx-pro-6000 -- ifconfig -a` fails with
    // `/sbin/ifconfig: No such file or directory`, because Ubuntu ships
    // iproute2 and not net-tools, so the entry above answers for the macOS
    // hosts and for no other kind of machine in the fleet.
    ApprovedCommand {
        argv: &["/usr/bin/ip", "addr"],
        why: "lists every network interface on a Linux host with the addresses it carries — the \
              same fact the `ifconfig -a` entry above reads, on the hosts where that entry \
              cannot run. Ubuntu ships iproute2 and not net-tools, so \
              `host exec ubuntu-server-rtx-pro-6000 -- ifconfig -a` answers \
              `/sbin/ifconfig: No such file or directory` and the fleet's one approved way to \
              read a host's interfaces was a macOS-only read; the address of the fleet's only \
              Linux host had to be inferred from `tailscale netcheck` instead, which reports \
              the reflexive address a relay observed and not one word about what the \
              interfaces on the machine actually hold. `addr` with no object and no operand is \
              iproute2's read-only listing form: every form that changes an address takes \
              `add`, `del`, `change`, `replace` or `flush` after it, none of which is in this \
              table and none of which can be appended, because the allowlist matches an entry \
              exactly and never appends operator words. What it prints are the addresses the \
              registry already holds",
    },
    // The three sign-in repairs, added 2026-09-02. These are the only entries
    // in this table that change anything, and they are here because the thing
    // they change cannot be reached any other way: a provider grant the vendor
    // has disowned is replaced by one browser sign-in, that sign-in belongs to
    // Brama's own CLI on the host whose vault the gateway reads, and the vault
    // that matters is never this control plane's. `brama-sub-wisent-app-codex-primary`
    // was recorded `needs_reauthorization` on 2026-08-27 with the provider's own
    // sentence -- "Your session has ended. Please log in again." -- and from that
    // moment every model call the fleet routed through that gateway had one live
    // provider and no way for an operator to repair it without a private ssh
    // session outside the registry-authorized channel. Each entry names one
    // provider, one exact Weles sign-in row, and its own fixed reason. The row
    // is named rather than inferred because Weles holds seven codex accounts
    // and two claude ones, and the cost of getting that wrong is one real
    // sign-in into somebody else's account; the reason is fixed because it is
    // recorded in Brama's journal beside the verdict and an operator-supplied
    // one would be an operator-supplied argument.
    //
    // The login budget is deliberately above the ten minutes the reauth
    // trajectory's own `login.mjs` allows itself. Setting the two equal, which
    // this table did first, meant the outer kill landed in the same second as
    // the inner cap: Weles answered 502 with no run detail and the trajectory
    // was SIGKILLed before it could write the page it was stuck on, which is
    // the one artifact the operator actually needs. The inner cap must be the
    // one that fires.
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
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
        ],
        why: "asks Weles to sign the codex account in on the host that holds the vault, then \
              proves the repair by Brama's own refresh. The row is the one Weles declares \
              primary for codex and maps to `brama-sub-wisent-app-codex-primary`, which is \
              the subscription the provider disowned; it is also the row Brama's own renewal \
              sweep already drives, so this entry cannot reach an account that sweep would \
              not. It changes exactly one thing: that subscription's stored provider \
              credential. It cannot spend money, because a sign-in buys nothing. No \
              credential reaches this command: Weles writes what it mints into the vault \
              directly, the admission bearer is acquired on the host, and the verdict this \
              prints carries a result, a reason and a login row and never a secret",
    },
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "subscription",
            "sign-in",
            "claude-code",
            "--login-item",
            "claude-wisent-google-sso",
            "--reason",
            "claude-code-vault-row-yields-no-credential-second-live-provider",
            "--login-timeout-ms",
            "900000",
            "--json",
        ],
        why: "the same repair for claude-code, whose stored document is account metadata \
              carrying no credential material: its pool contributes no model at all, and a \
              sign-in is what would put a credential there. A gateway with one live provider \
              is a gateway that stops serving at the next lapsed session, which is the state \
              this fleet was in on 2026-08-27. The row is Weles's declared primary for \
              claude, mapped to `brama-sub-wisent-app-claude-primary`. Same guarantees as \
              the codex entry: no argument, no purchase, no secret in argv or output",
    },
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "subscription",
            "sign-in",
            "kimi",
            "--login-item",
            "kimi-lukasz-google-sso",
            "--reason",
            "kimi-vault-row-yields-no-credential-second-live-provider",
            "--login-timeout-ms",
            "900000",
            "--json",
        ],
        why: "the same repair for kimi, in the same state as claude-code: a stored document \
              with no credential material and a pool that contributes no model. The row is \
              Weles's only kimi account and its declared primary, mapped to \
              `brama-sub-wisent-app-kimi-primary`. Same guarantees as the codex entry",
    },
    // What the installed Kimi CLI actually accepts, added 2026-09-02. Weles's
    // kimi login trajectory spawns `kimi login --json` and the CLI on
    // charless-mac-mini answers `error: unknown option '--json'`, so the run
    // never reaches an authorize URL and kimi has renewed nothing. Fixing a
    // trajectory against a flag list guessed from a pinned version is how that
    // mismatch happened; these three reads are how it gets fixed against the
    // binary that is really there.
    ApprovedCommand {
        argv: &[KIMI_CLI, "--version"],
        why: "prints the installed Kimi CLI version. It takes no argument, reads no session \
              and writes nothing; the version is the first thing a trajectory-versus-CLI \
              mismatch has to be judged against",
    },
    ApprovedCommand {
        argv: &[KIMI_CLI, "--help"],
        why: "prints the CLI's own subcommand list. `--help` short-circuits before any \
              subcommand runs, so nothing logs in, nothing is written, and no account state \
              is read",
    },
    ApprovedCommand {
        argv: &[KIMI_CLI, "login", "--help"],
        why: "prints the flags the `login` subcommand accepts. This is the exact question the \
              broken trajectory needs answered -- whether a machine-readable output flag \
              exists and what it is spelled -- and `--help` is answered by the argument \
              parser before the subcommand body, so no login is started and no browser \
              opens",
    },
    // The proof the sign-in entries above are judged by, added 2026-09-02. A
    // repaired credential that redeems is not a repaired gateway: the vault can
    // yield a value the provider then refuses, which is exactly the state
    // 2026-08-27 left, and only a real completion separates the two. It runs
    // through the same subscription dispatch a caller reaches, on the host, so
    // no bearer of any kind crosses this channel.
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "test",
            "--model",
            "codex/gpt-5.3-codex-spark",
            "--agent-id",
            "wisent-app",
            "--allow-provider-cost",
        ],
        why: "sends one fixed prompt through Brama's subscription dispatch and prints the \
              model, the token counts and the latency. The route is the provider's own \
              cheapest codex model -- the one its plan-probe table already names -- and the \
              request is covered by the subscription the account already holds, so it buys \
              nothing, renews nothing and raises no limit. `--allow-provider-cost` is \
              Brama's own acknowledgement flag and is fixed here because this entry exists \
              to spend exactly one completion; the prompt and the agent are compile-time \
              constants, and the answer carries no credential",
    },
    // The four reads a release that is installed but not running needs, added
    // 2026-09-02. On that evening `com.wisent.weles-admission` on
    // charless-mac-mini crash-looped on `Cannot find module
    // .../runtime/dist/worker/dispatch.js` while `stado release status
    // weles-worker` reported 0.5.57 committed and active. Three different
    // repairs hid behind that: the build had dropped the file, the install had
    // put it where the launcher does not look, or the launcher was resolving a
    // tree from an older release. Separating them is four facts about one
    // directory -- which digest `current` resolves to, which digests are
    // installed beside it, what the launcher sees inside the one it reaches,
    // and whether the compiled worker modules the API server imports are
    // there -- and this table could read none of them: `ls` existed only as
    // the fixed `ls /Applications`, and there is no `readlink`, `cat`, `find`
    // or `stat` entry. The whole diagnosis stopped on the symlink evidence
    // and said so.
    //
    // Each entry names the one service, because a path an operator supplies is
    // a path that can be `~/.ssh/id_ed25519`. `.stado/services` holds the
    // fleet's own installed release trees and nothing of the account's: no
    // documents, no keys, no credential files, and a directory name is not a
    // secret. All four are relative and are resolved by
    // [`home_rooted_script`] against the managed account's own home.
    ApprovedCommand {
        argv: WELES_ADMISSION_CURRENT,
        why: "prints the release directory `com.wisent.weles-admission` executes through. \
              The unit's program is that link plus a platform directory, so this name is the \
              whole answer to which release is running, and it is the fact `release status` \
              cannot give: that verb reports what a rollout recorded, and on 2026-09-02 the \
              two disagreed by four releases. `readlink` reads one link and writes nothing; \
              the path is a compile-time constant naming this one managed service",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_VERSIONS,
        why: "lists every release directory installed for that service beside the `current` \
              link, and, because `-l` renders a symlink with its target, the link and the \
              directory it names on the same page. This is what separates 'the rollout never \
              installed the release' from 'it installed it and left the link behind': the \
              installer keeps the previous version rather than deleting it, so a digest \
              present but unlinked is a rollback that happened and a digest absent is a \
              rollout that did not. `-l` is a display flag, the directory is fixed, and `ls` \
              writes nothing",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_RELEASE_TREE,
        why: "lists the inside of the release directory the launcher actually stands in: the \
              `payload` archive it unpacks from, the `runtime` tree it unpacks into, and the \
              modification times of both. That launcher unpacks only when the runtime carries \
              no ready marker, so a runtime older than its own payload is a tree pinned \
              incomplete, and a payload that is gone means the tree can never be re-derived \
              at all -- the difference between a release that will heal on the next start and \
              one that cannot. `-l` is a display flag, the directory is fixed, and `ls` writes \
              nothing",
    },
    ApprovedCommand {
        argv: WELES_ADMISSION_WORKER_MODULES,
        why: "lists the compiled worker modules in the runtime tree that service's launcher \
              actually resolves -- the directory the API server imports `dispatch.js` from, \
              reached through `current` exactly as the running process reaches it. A payload \
              proven to contain the file proves nothing about the tree under `current` if the \
              link points at a different release, which is the mistake this entry exists to \
              stop. It takes no flag and no operator path, lists names only, and writes \
              nothing",
    },
    // The three reads a web product's release and its unit need before either
    // one runs, added 2026-09-02. `stado web` builds a Node product on a fleet
    // builder with `npm ci` and runs it on a fleet host with `npm run start`,
    // and the public hostname in front of it is terminated by a
    // registry-managed Caddy unit. Each of those three facts is a property of
    // the machine that is true before the release is submitted, and none of
    // them could be asked of a host through this channel.
    //
    // All three probe absolute paths rather than the login shell's PATH, the
    // way the `uv --version` entry above does and for the same reason: a
    // non-interactive ssh login reads no profile, so a PATH lookup answers
    // `not found` on a host that carries the binary, which is the wrong answer
    // to a precondition check and the most expensive kind of wrong answer to
    // get.
    ApprovedCommand {
        argv: &[NODE_RUNTIME, "--version"],
        why: "prints the Node runtime's version, probing the absolute paths this fleet installs \
              it at rather than the login shell's PATH — which is a different question and \
              answers `not found` on a host that has the binary, because a non-interactive ssh \
              login reads no shell profile and the fleet's Node comes from Homebrew. A web \
              product's release builds with `npm ci` on whichever host the recipe's \
              `runner_platform` selects, and its unit runs `npm run start` on whichever host \
              the product is declared against, so a machine carrying no Node toolchain fails \
              the first inside a quality gate and the second at unit bootstrap. Until this \
              entry there was no sanctioned way to ask either host whether it has a Node \
              toolchain at all: the question got answered by reading a release log after a \
              build had already failed, which spends a whole submit to learn one fact that was \
              true of the machine before the release started. `--version` takes no argument, \
              installs nothing, resolves no registry, and runs no package script",
    },
    ApprovedCommand {
        argv: &[NPM_CLI, "--version"],
        why: "prints the npm CLI's version, probed the same way, for the other half of the same \
              precondition. The runtime and the package manager are separate binaries, a \
              partial or hand-rolled install leaves a host with one and not the other, and it \
              is npm — not node — that a web release invokes: `npm ci` in the quality gate and \
              `npm run start` in the unit. Its version is also the fact that decides whether \
              `npm ci` can read the product's checked-in `package-lock.json` at all, since a \
              lockfile written by a newer npm than the host carries is refused rather than \
              honoured. That is the difference between 'this builder cannot build a Node \
              product' and 'this product's build is broken', and before this entry the fleet \
              learned which one it was facing from a failed release's log. `--version` prints \
              and exits: it contacts no registry, writes no cache, and runs no lifecycle \
              script",
    },
    ApprovedCommand {
        argv: &[CADDY_PROXY, "version"],
        why: "prints the version of the Caddy binary a host carries, probed at the same \
              absolute paths rather than through the login shell's PATH. The public web edge \
              terminates TLS for a product hostname with a registry-managed Caddy unit, so \
              whether a host already carries that binary is the precondition of installing it: \
              a host that has it needs a unit and a configuration written for it, and a host \
              that does not needs the binary itself first, which is a different repair by a \
              different mechanism. Asking after the fact means learning the answer from a unit \
              that will not start, with the hostname already published and no certificate \
              behind it. `version` is Caddy's own read-only subcommand: it loads no \
              configuration, binds no port and starts no server, unlike `run`, `start` and \
              `reload`, none of which is in this table",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "list-units",
            "--type",
            "service",
            "--all",
            "--no-pager",
            "--no-legend",
        ],
        why: "lists this host's systemd services, the Linux counterpart of the `launchctl \
              list` entry above. Added 2026-09-03: the fleet's one linux-amd64 builder had \
              been running a two-day-old stado image that refuses today's registry document \
              (`policy:ValueError`), so its own janitor never learned a low watermark and it \
              claimed nothing -- every release build for that platform queued behind it. \
              Naming the unit that holds that process is the first step of the repair, and \
              this table could not name a systemd unit at all: `launchctl list` answers only \
              on macOS. `list-units` is systemd's read-only verb with every selector fixed \
              here; the mutating verbs (start, stop, restart, enable, daemon-reload) are \
              absent from this table and cannot be reached through it",
    },
    ApprovedCommand {
        argv: &[
            "/usr/bin/systemctl",
            "list-unit-files",
            "--type",
            "service",
            "--no-pager",
            "--no-legend",
        ],
        why: "lists the systemd service unit FILES installed on this host, which is a \
              different question from `list-units` above: a unit whose file exists but was \
              never loaded appears only here, and that is exactly the shape an undeclared \
              queue agent takes. Read-only, every selector fixed, and it takes no unit name",
    },
];

/// Every approved spelling, comma-separated, for help and error text.
pub fn allowlist() -> String {
    APPROVED_COMMANDS
        .iter()
        .map(ApprovedCommand::display)
        .collect::<Vec<String>>()
        .join(", ")
}

/// True when a word contains nothing a shell would act on.
pub fn is_shell_safe(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|character| {
            character.is_ascii_alphanumeric() || SAFE_PUNCTUATION.contains(character)
        })
}

/// Resolve the operator's words to an approved entry, or refuse.
///
/// Every refusal here is stated, not guessed: it is the one place that knows
/// the words matched nothing, so it says so with its own code (see
/// [`ExecRefusal`]). The approved spellings still reach the operator — an
/// operator who guessed wrong should not have to go read the source — but
/// they ride [`ExecRefusal::help`] rather than the sentence, because a
/// refusal that quotes its own help text is a refusal that can be
/// misclassified by it.
pub fn approve(words: &[String]) -> Result<&'static ApprovedCommand, ExecRefusal> {
    if words.is_empty() {
        return Err(ExecRefusal::unapproved("no command given".to_string()));
    }
    for word in words {
        if !is_shell_safe(word) {
            return Err(ExecRefusal::unapproved(format!(
                "argument {} contains a character a shell would interpret; \
                 host exec is an allowlist, not a shell",
                py_str_repr(word),
            )));
        }
    }
    let requested = words.join(" ");
    APPROVED_COMMANDS
        .iter()
        .find(|candidate| candidate.display() == requested)
        .ok_or_else(|| {
            ExecRefusal::unapproved(format!(
                "{} is not an approved host-exec command",
                py_str_repr(&requested),
            ))
        })
}

/// The remote program for an entry whose binary is installed at a different
/// absolute path on each platform: run the first candidate that is executable
/// on the host, with the entry's own fixed arguments.
///
/// Every word of this script is a compile-time constant of this module — the
/// candidate paths and the entry's arguments — and each one is quoted for the
/// remote shell anyway. The operator's words selected the entry and reach the
/// host in nothing else, so barrier three of this module holds exactly as it
/// does on the [`host_channel::run_program`] path.
///
/// A host carrying none of the candidates gets the refusal named here rather
/// than a shell's `No such file or directory` against whichever path happened
/// to be listed first, because the second reads as "the fleet installed this
/// wrongly" when the truth is "this program is not on this machine".
///
/// Every candidate's directory goes on `PATH` before the exec, and that is not
/// convenience. `/opt/homebrew/bin/npm` is a JavaScript shim whose first line
/// is `#!/usr/bin/env node`, so executing it on a channel whose `PATH` does
/// not carry Homebrew answers `env: node: No such file or directory` — which
/// is what `stado host exec charless-mac-mini -- npm --version` answered on
/// 2026-09-03 while `node --version` on the same host answered `v25.9.0` from
/// the directory beside it. The interpreter a shim needs is always a sibling
/// of the shim, so the directories this table already names are exactly the
/// ones that make it runnable. They are prepended, not appended: a host with
/// two Node installations must resolve the shim against the one whose path
/// this entry selected, not against whatever the login shell prefers.
fn candidate_script(candidates: &[&str], arguments: &[&str]) -> String {
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    let mut script = String::from("set -eu\n");
    // Reversed, because each line prepends: emitting the candidates back to
    // front leaves the first candidate's directory first on PATH, which is the
    // same precedence the exec loop below applies.
    for candidate in candidates.iter().rev() {
        if let Some(directory) = std::path::Path::new(candidate).parent() {
            let directory = directory.to_string_lossy();
            if !directory.is_empty() {
                script.push_str(&format!(
                    "PATH={}:\"$PATH\"\n",
                    shlex_quote(directory.as_ref())
                ));
            }
        }
    }
    script.push_str("export PATH\n");
    for candidate in candidates {
        // A candidate may be home-relative: the installers that lay these
        // programs down (`~/.stado/bin/install-cua-driver`, rustup, a global
        // npm prefix) write into the login user's home, and only the host
        // knows what that path is. `home_anchored` expands nothing here — it
        // emits the host's own `"$HOME"` followed by the quoted remainder,
        // exactly as the account-owned entries already do.
        let path = home_anchored(candidate);
        let marker = shlex_quote(&format!("{RESOLVED_EXECUTABLE_MARKER}{candidate}"));
        script.push_str(&format!(
            "if [ -x {path} ]; then printf '%s\\n' {marker} >&2; exec {path} {fixed}; fi\n"
        ));
    }
    script.push_str(&format!(
        "printf '%s\\n' {} >&2\nexit 127\n",
        shlex_quote(&format!(
            "this program is installed at none of its approved paths on this host: {}",
            candidates.join(", ")
        ))
    ));
    script
}

fn extract_resolved_executable(
    stderr: &mut String,
    candidates: &[&str],
) -> Result<Option<String>, DeployError> {
    let mut resolved: Option<String> = None;
    let mut retained = String::with_capacity(stderr.len());
    for segment in stderr.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(path) = line.strip_prefix(RESOLVED_EXECUTABLE_MARKER) {
            if path.is_empty()
                || !candidates.contains(&path)
                || resolved.replace(path.to_string()).is_some()
            {
                return Err(DeployError(
                    "host returned an invalid resolved executable marker".into(),
                ));
            }
        } else {
            retained.push_str(segment);
        }
    }
    let Some(resolved) = resolved else {
        return Ok(None);
    };
    *stderr = retained;
    Ok(Some(resolved))
}

/// Run one approved command on a canonical registry host.
///
/// The error type is [`ExecRefusal`] rather than [`DeployError`] so the
/// allowlist's own refusal keeps the code it stated all the way to the
/// operator. Every other failure on this path converts in through
/// `From<DeployError>` with no code, which is the honest answer for a
/// sentence produced by ssh, the registry or the remote shell.
pub async fn exec_host(
    target_name: &str,
    words: &[String],
    runner: &Runner,
) -> Result<Value, ExecRefusal> {
    // Refuse before resolving the target: an operator who typed something
    // outside the allowlist gets the allowlist back immediately, without a
    // registry round-trip and without the host ever being contacted.
    let approved = approve(words)?;
    let target = host_channel::canonical_target(target_name).await?;
    // A program that lives in exactly one place keeps the plain transport: its
    // fixed argv IS the command line, and probing a single path on the host
    // would only replace the remote shell's own report of a missing program
    // with a worse one.
    let account = account_program(approved.argv.first().copied().unwrap_or_default());
    let candidates = match account {
        Some(account) => account.candidates,
        None => approved.candidates(),
    };
    // `mut`: a multi-candidate run reports which path it execed on stderr, and
    // that marker line is consumed out of the operator-visible stderr below.
    let mut output = match (approved.argv.split_first(), account) {
        (Some((_, arguments)), Some(account)) => {
            let script = account_script(account, arguments);
            host_channel::run_script_with_timeout(
                &target,
                &script,
                Duration::from_secs(account.timeout_seconds),
                runner,
            )
            .await?
        }
        // A read whose fixed paths are relative to the managed account's home
        // stands in that home first. One candidate, one absolute program, so
        // nothing below has a marker to look for.
        (Some(_), None) if home_rooted(approved.argv) => {
            let script = home_rooted_script(approved.argv);
            host_channel::run_script(&target, &script, runner).await?
        }
        (Some((_, arguments)), None) if candidates.len() > usize::from(true) => {
            let script = candidate_script(candidates, arguments);
            host_channel::run_script(&target, &script, runner).await?
        }
        _ => host_channel::run_program(&target, approved.argv, runner).await?,
    };
    // Which path the host actually execed. Only the multi-candidate script
    // reports it: the account script resolves `$program` in the remote shell
    // and prints no marker, so that path has nothing to report here and
    // asking it for one would fail a run that worked.
    let resolved_executable = if account.is_some() {
        None
    } else if candidates.len() > usize::from(true) {
        match extract_resolved_executable(&mut output.stderr, candidates)? {
            Some(path) => Some(path),
            // A failed run may never have reached any candidate.
            None if !output.ok() => None,
            None => {
                return Err(
                    DeployError("host returned no resolved executable marker".into()).into(),
                )
            }
        }
    } else {
        candidates.first().copied().map(str::to_string)
    };

    let ok = output.ok();
    // Read the remote's own last line before the body is moved into the
    // receipt.
    let error = (!ok).then(|| host_channel::last_error_line(&output, "ssh failed"));
    let receipt = HostExecReceipt {
        schema: "stado.host-exec-receipt.v1".into(),
        target: target.name,
        ssh: target.ssh,
        ssh_fallbacks: target.ssh_fallbacks,
        command: approved.display(),
        argv: approved
            .argv
            .iter()
            .map(|word| (*word).to_string())
            .collect(),
        program_candidates: (candidates.len() > usize::from(true))
            .then(|| candidates.iter().map(|path| (*path).to_string()).collect()),
        resolved_executable,
        timeout_seconds: account.map(|account| account.timeout_seconds),
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.code,
        status: if ok {
            OK_STATUS.into()
        } else {
            host_channel::FAILED_STATUS.into()
        },
        error,
    };
    serde_json::to_value(receipt).map_err(|error| DeployError(error.to_string()).into())
}

#[cfg(test)]
mod tests {
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

    /// Every entry must be reachable by the spelling it advertises.
    ///
    /// This is the trap the home-rooted reads were written around: an entry
    /// carrying a `~/…` argument advertises a spelling barrier one refuses,
    /// so it would sit in the table forever, listed in every refusal, and
    /// never run.
    #[test]
    fn every_advertised_spelling_selects_its_own_entry() {
        for entry in APPROVED_COMMANDS {
            let words: Vec<String> = entry.display().split(' ').map(str::to_string).collect();
            for word in &words {
                assert!(
                    is_shell_safe(word),
                    "{}: the word {word:?} an operator must type is refused by barrier one",
                    entry.display()
                );
            }
            let selected = approve(&words).expect("its own spelling selects it");
            assert_eq!(selected.argv, entry.argv, "{}", entry.display());
        }
    }

    #[test]
    fn every_entry_states_why_it_is_safe() {
        for entry in APPROVED_COMMANDS {
            assert!(
                entry.why.len() > 40,
                "{}: an entry without a defensible reason does not belong in the table",
                entry.display()
            );
        }
    }

    /// The three service-tree reads name one service and no operator path.
    #[test]
    fn the_service_tree_reads_are_home_rooted_and_carry_no_absolute_path_argument() {
        for argv in HOME_ROOTED_READS {
            assert!(
                APPROVED_COMMANDS.iter().any(|entry| entry.argv == *argv),
                "{argv:?} is home-rooted but is not in the allowlist"
            );
            let (program, arguments) = argv.split_first().expect("a program");
            assert!(
                program.starts_with('/'),
                "{program} must be an absolute system program"
            );
            for argument in arguments {
                assert!(
                    !argument.starts_with('/') || argument.starts_with("-"),
                    "{argument} would escape the managed account's home"
                );
                assert!(
                    !argument.contains(".."),
                    "{argument} would climb out of the service tree"
                );
            }
        }
    }

    #[test]
    fn a_home_rooted_read_stands_in_the_account_home_before_it_runs() {
        let script = home_rooted_script(WELES_ADMISSION_CURRENT);
        assert_eq!(
            script,
            "set -eu\ncd \"$HOME\"\nexec /usr/bin/readlink \
             .stado/services/weles-admission/current\n"
        );
    }

    /// The reads that were unavailable on 2026-09-02 are the reads that now
    /// exist, addressed the way the running unit addresses the same tree.
    #[test]
    fn the_admission_reads_reach_current_the_way_the_unit_does() {
        assert_eq!(
            approve(&[
                "readlink".into(),
                ".stado/services/weles-admission/current".into()
            ])
            .expect("approved")
            .argv,
            WELES_ADMISSION_CURRENT
        );
        assert_eq!(
            approve(&[
                "ls".into(),
                "-l".into(),
                ".stado/services/weles-admission".into()
            ])
            .expect("approved")
            .argv,
            WELES_ADMISSION_VERSIONS
        );
        // Through `current`, not through a pinned digest: a read that named
        // the digest would answer for a tree the unit may not be running.
        assert!(
            WELES_ADMISSION_WORKER_MODULES[1]
                .starts_with(&format!("{WELES_ADMISSION_SERVICE_DIR}/current/")),
            "{:?}",
            WELES_ADMISSION_WORKER_MODULES
        );
        assert!(WELES_ADMISSION_WORKER_MODULES[1].ends_with("/runtime/dist/worker"));
    }

    /// A path an operator supplies is a path that can be a private key.
    #[test]
    fn no_entry_can_be_pointed_at_a_home_dotfile() {
        for entry in APPROVED_COMMANDS {
            for word in entry.argv {
                assert!(
                    !word.contains(".ssh"),
                    "{}: reads inside .ssh are not approvable",
                    entry.display()
                );
            }
        }
        assert!(approve(&["cat".into(), ".ssh/id_ed25519".into()]).is_err());
        assert!(approve(&["readlink".into(), ".stado/services/brama/current".into()]).is_err());
    }
}

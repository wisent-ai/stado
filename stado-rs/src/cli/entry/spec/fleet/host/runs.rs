//! Working inside a delivered managed run tree on a host, and on the host's
//! own Stado configuration and periodic table. The second block of
//! `stado host --help`.

use clap::Subcommand;

/// The second block of `stado host` verbs. Flattened into
/// `super::HostCommands`, so splitting the declaration across files changes
/// no command line.
#[derive(Subcommand)]
pub(crate) enum HostRunCommands {
    /// Run one approved command on TARGET (allowlist, not a shell). Every
    /// entry is read-only except the declared provider sign-in repairs.
    ///
    /// Retained Tailscale logs are available without changing logging settings,
    /// restarting a service, or opening a test network connection.
    ///
    /// macOS: log show --last 1h --style compact --info --debug --no-pager
    /// --process Tailscale --process IPNExtension
    /// --process io.tailscale.ipn.macsys.network-extension --process tailscaled
    ///
    /// Linux: journalctl --unit tailscaled --since -1h --no-pager --output short-iso
    ///
    /// These commands retain the native timestamps and messages. Empty output
    /// does not establish that Funnel works. Missing tools and access refusals
    /// remain command failures. Changed arguments or an extra process, path, or
    /// time window are refused before the host is contacted.
    Exec {
        target: String,
        /// Emit the report as JSON instead of the host's raw output.
        #[arg(long)]
        json: bool,
        /// The approved command, after `--`. Run with an unapproved one to
        /// see the allowlist.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Deliver one local file or directory into a canonical run directory on TARGET.
    ///
    /// DESTINATION is relative to the registry-approved account's home and
    /// must be below `.stado/work/runs/<canonical lowercase UUID>/`. The
    /// command refuses a missing, special, or root-symlink SOURCE; an empty or
    /// malformed file list; and any destination outside that shape before
    /// contacting TARGET. On TARGET it refuses symlinked, foreign-owned, or
    /// wrong-kind destination state before rsync transfers a byte. The
    /// destination is replaced atomically after the transfer.
    Deliver {
        /// Canonical Stado target selector.
        target: String,
        /// Local regular file, directory, or application bundle.
        source: String,
        /// Managed path relative to the target account's home.
        destination: String,
        /// Read a nonempty NUL-delimited list of relative SOURCE paths from
        /// PATH, or '-' for stdin. The final path must end with NUL.
        #[arg(long, value_name = "PATH")]
        files_from: Option<String>,
        /// Emit the delivery receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Build one declared Cargo binary inside a delivered managed run tree.
    ///
    /// The command is fixed to `cargo build --locked --release`; only the
    /// manifest and binary name vary. The manifest must resolve below the
    /// selected account's `$HOME/.stado/work/runs`.
    Build {
        target: String,
        /// Absolute Cargo.toml path inside a delivered managed run.
        #[arg(long)]
        manifest_path: String,
        /// Declared Cargo binary target to build.
        #[arg(long = "bin")]
        binary: String,
        /// Capture Cargo's stdout, stderr, and exit status in one JSON receipt.
        #[arg(long)]
        json: bool,
    },
    /// Run one executable from a managed run tree with this process's standard
    /// input, output, and error attached.
    ///
    /// SIGHUP, SIGINT, and SIGTERM received by Stado are forwarded to the
    /// remote program. Arguments are ordinary process arguments; sensitive
    /// input belongs on stdin and never in `--arg`.
    #[command(name = "run-attached")]
    RunAttached {
        target: String,
        /// Absolute executable path below `$HOME/.stado/work/runs`.
        #[arg(long)]
        program: String,
        /// Non-secret program argument; repeat for each argument.
        #[arg(long = "arg", action = clap::ArgAction::Append, allow_hyphen_values = true)]
        arguments: Vec<String>,
        /// Capture the program's streams and exit status in one JSON receipt
        /// instead of forwarding its output live.
        #[arg(long)]
        json: bool,
    },
    /// Recursively remove one complete managed run directory.
    ///
    /// PATH must be one direct child of `$HOME/.stado/work/runs`. Absence is a
    /// retry-safe success; symlinks, foreign ownership, the shared root, and
    /// nested subdirectories are refused.
    #[command(name = "remove-run-directory")]
    RemoveRunDirectory {
        target: String,
        /// Absolute managed run directory on TARGET.
        path: String,
        /// Emit the removal receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read TARGET's crontab, and optionally prune one entry from it.
    ///
    /// The periodic table is the one place a fleet host can declare a
    /// process that no launchd domain and no registry document mentions.
    /// charless-mac-mini carried four `@reboot` entries outside both, two of
    /// which restart duplicates that had just been retired with verified
    /// postconditions — so every repair on that host was one reboot from
    /// coming back, and the only way to change the table was a bare
    /// `crontab -e` over ssh.
    ///
    /// `--prune` previews by default and refuses anything but a single
    /// matching line that references `$HOME/.stado`; `--apply` saves the
    /// whole table under `$HOME/.stado/cron-backups` first and prints the
    /// `--restore` command that puts it back.
    Cron {
        target: String,
        /// Literal text naming the ONE entry to remove; usually the script's
        /// path. Refused when it reaches more than one line.
        #[arg(long)]
        prune: Option<String>,
        /// Install a table saved by an earlier `--prune --apply`.
        #[arg(long, conflicts_with = "prune")]
        restore: Option<String>,
        /// Change the table. Without it, `--prune` only reports what it would
        /// remove.
        #[arg(long)]
        apply: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deliver the checked-in Weles receipt-trust renderer to TARGET and print
    /// the public five-field Spis receipt-trust document it builds from
    /// TARGET's own live Skarbiec. The admission authority's private half
    /// never leaves the host.
    #[command(name = "render-spis-admission-trust")]
    RenderSpisAdmissionTrust {
        target: String,
        /// Local renderer to deliver and run.
        source: String,
    },
    /// Report TARGET's stado-managed binaries, fixed Cargo-home metadata and
    /// bin membership, forward markers and loopback listeners, and whether
    /// each marker still matches a live listener.
    Inventory {
        target: String,
        /// Emit the inventory and its reconciliation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read TARGET's effective Stado configuration through its fleet channel.
    ConfigShow { target: String },
    /// Persist one dotted Stado configuration value on TARGET.
    ConfigSet {
        target: String,
        key: String,
        /// JSON value, or a bare string as accepted by `stado config set`.
        value: String,
        /// Reconcile this registry-managed service after the atomic write so
        /// long-lived processes observe the new configuration immediately.
        #[arg(long)]
        reload_service: Option<String>,
    },
    /// Remove one dotted Stado configuration key from TARGET.
    ///
    /// A declaration that should never have been made is retracted, not
    /// overwritten with a null: a key present with a null value and a key that
    /// is absent read the same through `jq` and differently through the code
    /// that iterates the object.
    #[command(name = "config-unset")]
    ConfigUnset {
        target: String,
        key: String,
        /// Reconcile this registry-managed service after the atomic write so
        /// long-lived processes observe the retraction immediately.
        #[arg(long)]
        reload_service: Option<String>,
    },
}

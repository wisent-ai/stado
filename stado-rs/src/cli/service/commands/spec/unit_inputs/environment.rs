//! The environment, endpoint and grant verbs of `stado service`.

use clap::Subcommand;

/// The third block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum EnvironmentCommands {
    /// One managed unit's environment: show it, set or unset one key, and
    /// check the endpoints its env file declares.
    Env {
        #[command(subcommand)]
        command: ServiceEnvCommands,
    },

    /// Is the DECLARED unit the process on its own port?
    ///
    /// `show` reports the unit declaration; `env check` reports whether
    /// anything answers on a declared port. Neither establishes that the
    /// listener belongs to this unit. Another job can use the same argument
    /// vector while holding the declared port.
    ///
    /// Ownership is decided by launchd label, never by argv. The pid
    /// holding each port is walked up its own parent chain until a pid appears
    /// in `launchctl list`, because a launcher script is the job and the
    /// server it starts is the child that holds the socket. A label that
    /// cannot be read — a system LaunchDaemon is invisible to an unprivileged
    /// `launchctl list` — is reported `unknown`, never as "nobody owns it".
    ///
    /// Verdicts are `serving`, `not_serving`, and `unknown` for a question
    /// that could not be answered; the third is never folded into either of
    /// the others. Exits non-zero on anything but `serving`; an unreadable
    /// owner cannot establish that the declared unit serves the port.
    Serving {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// One loopback port this unit is supposed to SERVE; repeat for each.
        ///
        /// Deliberately not taken from the unit's env file. That file names
        /// every endpoint the unit touches, and most of them are ports it
        /// CALLS — `STADO_API_URL`, `WC_SKARBIEC_URL` — owned by other
        /// services on purpose. Judging those as "this unit must own it" makes
        /// every healthy dependency a finding, which is how a check stops
        /// being read. `env check` is the command for dependencies; this
        /// one is about the ports the service itself answers on. Omit these
        /// and the service directory's declared endpoint for this host is
        /// used.
        #[arg(long = "port")]
        ports: Vec<u16>,
        #[arg(long)]
        json: bool,
    },
    /// Host primitive used by `env set`/`env unset` on a systemd unit: set
    /// (with --value-stdin, the base64 value on standard input) or remove one
    /// `Environment=` key in the file. The value never appears in argv, where
    /// every other process on the host could read it.
    #[command(name = "unit-env-local", hide = true)]
    UnitEnvLocal {
        #[arg(long)]
        path_b64: String,
        #[arg(long)]
        key_b64: String,
        #[arg(long)]
        value_stdin: bool,
        #[arg(long)]
        uid: u32,
    },
}

#[derive(Subcommand)]
pub enum ServiceEnvCommands {
    /// Show a managed unit's environment.
    ///
    /// Without `--env-file`: the effective environment the unit runs with,
    /// parsed from the plist or systemd unit and its drop-ins; a value whose
    /// variable name looks like a credential is replaced, in the table and in
    /// `--json` alike.
    ///
    /// With `--env-file` (and `--host`): the owner-controlled file a launcher
    /// `.`-sources, which on this fleet is where the interesting values live.
    /// Every assignment is listed in FILE ORDER with its line number, and a
    /// key assigned twice is reported twice — `effective` for the last
    /// assignment, `shadowed` for every earlier one — because a sourced file
    /// assigns top to bottom and a later duplicate silently wins. A value
    /// whose key looks like a credential is withheld, and a URL carrying
    /// userinfo is withheld whatever its key is called; an endpoint, a port,
    /// a flag or a `$REFERENCE` is shown whatever its key is called. The
    /// decision is made ON THE HOST, so a withheld value never crosses the
    /// channel.
    Show {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Registry host to read. Required with `--env-file`; without it,
        /// omit to read every host that manages the unit.
        #[arg(long)]
        host: Option<String>,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: Option<String>,
        /// With `--env-file`: show this one variable's value in full, whatever
        /// its name suggests. The key name travels; no secret is ever placed
        /// in a remote command line either way.
        #[arg(long, requires = "env_file")]
        reveal: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Replace one key in a managed runtime env file or systemd definition.
    ///
    /// The value is read from an owner-only local file and travels only inside
    /// the approved encrypted channel's request body.
    /// A systemd unit or its own .conf drop-in is updated in place; the manager
    /// reloads changed definitions without restarting the running service.
    Set {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        /// Absolute owner-only local file containing the value.
        #[arg(long)]
        value_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove one key from a managed env file or systemd definition.
    Unset {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Does this unit's env file agree with what is actually listening?
    ///
    /// Every loopback URL or port the file's effective assignments declare,
    /// checked against the host's own socket table, with the process that
    /// holds each port named. `host inventory` does this for forward markers;
    /// this does it for a unit's environment.
    ///
    /// Exits non-zero when a loopback endpoint is declared and nothing is
    /// listening there, and when the check could not be performed at all.
    Check {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },
}

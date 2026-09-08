//! `stado fleet ingress up|status|down` — the public entrance the one-line
//! invite mode needs, stood up by one command and with no Cloudflare account,
//! token or DNS record behind it.
//!
//! The one-line mode of [`crate::cli::fleet::invite`] has always had a
//! precondition it could report and never satisfy: the machine being added has
//! to reach an origin that serves `/join.sh`. `stado dashboard
//! --enrollment-only` made such an origin safe to publish — it answers three
//! routes and 404s everything else, before authorization, the store and the
//! vault — but publishing it was still two processes an operator started by
//! hand, a port they had to remember, and an address they had to read out of a
//! log. That is not a feature, it is a runbook, and a runbook is what nobody
//! executes at the moment somebody's laptop needs adding.
//!
//! So this is the entrance as a command. `up` picks its own loopback port,
//! starts the narrow listener on it, starts a Cloudflare quick tunnel in front
//! of it, waits for the address that tunnel prints, and then — the part that
//! makes the difference between a feature and a hope — fetches `/join.sh`
//! **from the internet, through that address** and compares what came back
//! with the script this very binary would have served. Only then is anything
//! published. A verification that did not pass is a teardown and an error
//! naming the stage that failed; it is never "it is probably up".
//!
//! ## What a quick tunnel is, said out loud
//!
//! `cloudflared tunnel --url http://127.0.0.1:PORT` needs no account, no API
//! token, no zone and no DNS record, and it hands back a `*.trycloudflare.com`
//! address. Cloudflare documents that mode as **not for production** and rate
//! limits it, and the address is **new on every start**. Both facts are printed
//! by `up` and by `status`, and `invite` repeats the second one whenever it
//! builds a one-liner on top of an ingress address: an invitation is a thing
//! somebody else runs later, and "later" is on the far side of any restart.
//!
//! For an entrance used a handful of times a month to add a machine, that trade
//! is the right one. For anything a service depends on, it is not, which is why
//! `--named` exists as a refusal rather than a second code path: the named mode
//! wants a Cloudflare API token, the vault has no such field, and a command
//! that pretends otherwise would fail three steps later with a Skarbiec error
//! nobody can act on.
//!
//! ## Why the processes outlive the command
//!
//! An entrance that dies with the terminal that opened it is not an entrance.
//! Both children are started as process-group leaders (`process_group(0)`), so
//! they survive this process and are not in the terminal's foreground group —
//! a Ctrl-C aimed at some later command cannot take the fleet's front door
//! down. The group id is the leader's pid, so the published object carries both
//! group ids and `down` signals the *group*: whatever `cloudflared` or the
//! listener spawned goes with them, instead of leaving a child holding the port
//! after its parent was killed.
//!
//! A pid outlives nothing reliably, so it is never trusted alone. `down` and
//! `status` read the leader's command line first and only act on a process that
//! still looks like the one that was started; a recycled pid is reported as
//! gone, not signalled.
//!
//! ## The components
//!
//! The seams are the components: `record` is the entrance as published and the
//! one read every subcommand starts from, `runtime` is what an entrance is made
//! of on this machine — binaries, port, process groups — `verify` is the part
//! that decides, and `command` is the three subcommands themselves. The
//! deadlines and the two mode names stay here, because the stages read them
//! and no stage owns them.

use std::time::Duration;

mod command;
mod record;
mod runtime;
mod verify;

pub use command::down::down;
pub use command::status::status;
pub use command::up::up;
pub use record::{ingress_document, parse_ingress, published, Ingress, PidHint};
pub use runtime::binaries::cloudflared_binary;

/// Where the published entrance lives. It sits under the join requests' prefix
/// with the invites, and like them it is not a join request: `fleet pending`
/// lists `enrollments/` and already skips every document it cannot parse as
/// one.
pub const INGRESS_PATH: &str = "enrollments/ingress.json";

/// The two tunnel modes. `quick` is the accountless `*.trycloudflare.com`
/// address; `named` is a tunnel on the fleet's own domain, which needs a
/// credential the vault does not have.
pub const MODE_QUICK: &str = "quick";
pub const MODE_NAMED: &str = "named";

/// Why `--named` is a refusal today, in one sentence, naming the field rather
/// than telling somebody to "configure Cloudflare".
///
/// The distinction matters: a named tunnel is not blocked by a setting nobody
/// filled in, it is blocked by a vault item that does not exist, and Skarbiec
/// refuses to grant on a field it cannot see. There is nothing to configure
/// until that item is created by whoever owns the Cloudflare account.
const NAMED_REFUSAL: &str =
    "--named cannot be established today: a named tunnel needs a Cloudflare \
     API token and the vault has no 'platform-admin-cloudflare#api_token' field, so Skarbiec \
     refuses to grant on it and no credential exists to authenticate the tunnel with";

/// Where Stado looks for `cloudflared` when nothing names it explicitly, in
/// order. Homebrew's prefix first because that is where it lands on the
/// operator machines this fleet is driven from; `/usr/local/bin` second for
/// Intel Homebrew and manual installs; `PATH` last, so a deliberately placed
/// binary still wins over nothing at all.
const CLOUDFLARED_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/cloudflared",
    "/usr/local/bin/cloudflared",
];

/// How long the loopback listener gets to answer its own port. It serves three
/// routes and touches neither store nor vault before answering `/join.sh`, so
/// this is generous by an order of magnitude and only ever spent when something
/// is actually wrong.
const LISTENER_DEADLINE: Duration = Duration::from_secs(15);
/// How long `cloudflared` gets to print the address it was given.
const TUNNEL_DEADLINE: Duration = Duration::from_secs(45);
/// How long the freshly minted name gets to appear in Cloudflare's own DNS.
/// Measured on this fleet's operator machine: about six seconds after
/// `cloudflared` prints the address. The allowance is an order of magnitude
/// wider because the cost of giving up early is a torn-down tunnel that was
/// about to work.
const DNS_DEADLINE: Duration = Duration::from_secs(90);
/// How long the published address then gets to answer `/join.sh`. The name
/// resolves by this point, so this is spent only on the edge finishing its own
/// propagation.
const PUBLIC_DEADLINE: Duration = Duration::from_secs(60);
/// Gap between DNS-publication polls. Deliberately slower than [`POLL`]: this
/// one asks a public resolver a question, and asking it three times a second
/// would be rude for no gain when the answer changes once.
const DNS_POLL: Duration = Duration::from_secs(2);
/// Gap between polls of the listener, tunnel and verification deadlines.
const POLL: Duration = Duration::from_millis(400);
/// Timeout of one external fetch during verification.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a signalled process group gets to go away before it is killed.
const TERMINATE_GRACE: Duration = Duration::from_secs(5);

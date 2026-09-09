//! Recovery of managed units on a host this login is not privileged to
//! bootstrap, run against the machine executing the test.
//!
//! What this area used to be, and why it was deleted rather than repaired: it
//! modelled a whole macOS host in shell. A script named `ssh` on PATH read
//! the product's remote program off stdin and piped it into a local `bash`
//! against stand-in `launchctl`, `plutil`, `PlistBuddy`, `pgrep`, `ps`,
//! `kill`, `sleep`, `sudo`, `stat`, `id` and `hostname` executables, over two
//! invented machines (`fake-mini`, `fake-agent-mini` at `approved@10.9.9.9`)
//! whose launchd state was three hand-written files — a `keepalive` word, a
//! `pids` table and a `program` line. Every sentence it asserted came out of
//! that fixture, so nothing in it was evidence about launchd, about this
//! product's privilege, or about a host.
//!
//! What it is now: one registry target naming THIS machine by its own kernel
//! host name, so `host_channel::target_is_this_host` is true and the
//! product's current-host path runs here; a real LaunchAgent in this login's
//! own `gui/<uid>` domain; and this machine's real `/bin/launchctl`,
//! `/usr/bin/plutil` and `/usr/libexec/PlistBuddy`. Nothing is substituted on
//! PATH. Every assertion reads state: the plist on disk, `launchctl print`,
//! `launchctl list`, the persisted registry document, the exit code, and the
//! refusal sentences — each copied from a live run of this fixture.
//!
//! Where each promise is proved:
//!
//! * `service restart` loads a declared unit launchd holds no job for, in the
//!   domain the resolver chose — `service.rs`;
//! * `service stop` boots the job out and keeps the file and the declaration,
//!   so a cutover is reversible — `service.rs`;
//! * the refusals: a system LaunchDaemon with no host account, a system
//!   daemon whose unit file the host does not have, and a host the registry
//!   does not declare — `refusals.rs`;
//! * the host repair pass reloading the managed beacon, and the blockers for
//!   a missing unit file, an incomplete scoped health configuration and a
//!   forbidden ambient credential — `recover.rs`.
//!
//! Not covered here, and deliberately not faked:
//!
//! * the unprivileged restart of a KeepAlive system daemon — ending the
//!   process launchd will replace — and the refusals that gate it (`KeepAlive`
//!   absent, `false`, conditional, or a process owned by another account).
//!   All four are read from a plist under `/Library/LaunchDaemons`, which is
//!   `root:wheel` and unwritable by this login, so producing any of those
//!   states needs root on this machine. Declaring another vendor's installed
//!   daemon instead would be a substitution, and the restart path attempts a
//!   privileged `launchctl kickstart -k` on whatever it is given, so it would
//!   also be a live command against somebody else's service.
//! * `needs_privileged_bootstrap` in the repair pass, for the same reason: it
//!   requires a readable unit file in launchd's system domain, and this host
//!   has none belonging to this fleet.
//!
//! `refusals.rs` proves the unprivileged half that IS reachable: the system
//! domain is named, the privileged step is refused, and the host is read
//! afterwards to show nothing happened.

mod fixture;
mod recover;
mod refusals;
mod service;

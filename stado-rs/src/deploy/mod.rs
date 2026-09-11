//! Deploy subsystem: operator-host provisioning.
//!
//! Port of `stado/deploy/` (`stado/deploy/__init__.py` is an empty package
//! marker — no runtime surface):
//!
//! - [`bootstrap`] — `stado bootstrap`: SSH-based remote provisioning of
//!   kind=local registry targets (pip install + inline systemd units).
//! - [`local_install`] — `stado bootstrap --local`: per-user launchd /
//!   systemd --user install on the current machine for the agent /
//!   coordinator / disk-cleanup / failure-fixer / watchdog kinds.
//! - [`host_recovery`] — the `stado repair stado --step host` implementation:
//!   a fixed, narrow SSH recovery program for managed macOS hosts, with the
//!   tab-delimited `STADO_*` marker protocol ported byte-exactly.
//! - [`host_users`] — `stado host user create`: account creation on
//!   registry hosts over SSH; the password travels only on SSH stdin.
//!
//! The read-only host commands of `stado.wisent.com/docs/missing-commands`
//! have NO Python original. They share one
//! channel, [`host_channel`], which is the option set and report shape of
//! [`host_reboot`] factored out:
//!
//! - [`host_uptime`] — `stado host uptime`: uptime, load averages, logins.
//! - [`host_ping`] — `stado host ping`: ssh reachability AND health-beacon
//!   age, combined into the worse of the two verdicts.
//! - [`host_disk`] — the reader behind `stado space report`: `df` plus the
//!   registry cleanup policy and the janitor's own recorded state.
//! - [`host_cleanup`] — the `registry_cleanup` stage behind `stado space
//!   reclaim`: drives the host's own janitor and contains no cleanup policy.
//! - [`host_exec`] — `stado host exec`: one command from a fixed
//!   allowlist, read-only apart from the declared provider sign-in
//!   repairs. Not a shell.
//! - [`host_inventory`] — `stado host inventory`: the stado-managed
//!   binaries, fixed Cargo-home metadata and bin membership, forward markers
//!   and loopback listeners of one host, plus the verdict on whether each
//!   marker still matches a live listener.
//!   It is NOT an `host_exec` allowlist entry because it reduces and caps
//!   every value it reads off the host; that table passes a program's
//!   output through untouched.
//! - [`host_object_relocate`] — `stado space relocate`: re-address objects
//!   from one key prefix to another INSIDE the store, on the host
//!   that holds it. The object API has no move and no server-side copy, so
//!   the alternative was pulling 134 MiB bodies through the control plane's
//!   loopback writer, which is what took that host's release ingress down.
//!   It previews by default and `--apply` verifies every destination before
//!   it unlinks a source.
//!
//! [`host_release`] is the one WRITE command in that group, and the only
//! thing in this crate that owns "get this build onto that host" — the gap
//! `ARCHITECTURE.md` names. It rides the same channel and follows Weles's
//! shipped auto-deploy order exactly: fetch the exact coordinate, verify it
//! against the operator's configured SHA-256, check the layout, stage it
//! under a versioned directory, and only then atomically repoint the active
//! binary and restart the declared unit. The three phases are three separate
//! programs on the channel, so "nothing activates before it verified" is
//! visible at the [`Runner`] seam rather than promised inside one script.
//!
//! [`host_link`] is not a command at all: it is the connectivity block a
//! host collects about ITSELF and publishes inside its health beacon, so
//! that "why did this machine go quiet" has an answer in the product
//! instead of in an operator's shell history.
//!
//! [`fleet_claim`] is not a command either: it is the fleet-level half of
//! [`host_gates`], reported wherever queued work is shown. `host gates`
//! answers "why is THIS host claiming nothing", one ssh round trip at a
//! time, which is only reachable by an operator who already suspects a
//! specific host. `fleet_claim` answers "can ANYTHING claim this queue" from
//! the store alone, in the same words, so `stado status` and `stado
//! overview` can state the one fact a queue listing cannot show: that a
//! queue with no claimant looks exactly like an empty one.
//!
//! Every subprocess is orchestrated through the [`Runner`] seam so tests
//! can inject a fake command runner and never spawn real
//! ssh/launchctl/systemctl. The production runner is
//! [`production_runner`] (tokio::process).
//!
//! `stado/deploy/templates/*.tmpl` (5 systemd units rendered by the
//! repo-root `install.sh` via sed) are NOT copied into the crate: the only
//! consumer is `install.sh`, which is not ported — `bootstrap.py` renders
//! its own inline units (see [`bootstrap`]).

pub mod artifact_install;
pub mod bootstrap;
pub mod fleet_claim;
pub mod fleet_vaults;
pub mod host_access;
pub mod host_backup_audit;
pub mod host_build_caches;
pub mod host_capability;
pub mod host_channel;
pub mod host_cron;
pub mod host_delivery;
pub mod host_disk;
pub mod host_exec;
pub mod host_gates;
pub mod host_gui_automation;
pub mod host_inventory;
pub mod host_link;
pub mod host_object_relocate;
pub mod host_precheck_runner;
pub mod host_reclaim;
pub mod host_recovery;
pub mod host_release;
pub mod host_run;
pub mod host_state;
pub mod host_storage_reconcile;
pub mod host_users;
pub mod inference;
pub mod local_install;
pub mod mobile_runtime;
pub mod native_signing;
pub mod products;
pub mod reconcile;
pub mod scratch;
pub mod service;
pub mod service_catalog;
pub mod service_env_file;
pub mod service_file_fetch;
pub mod service_label_print;
pub mod service_serving;
pub mod service_spawn_watch;
pub mod staged_release;
pub mod stream;
pub mod weles_browser_runtime;
pub mod weles_browser_task;
pub mod weles_capture;

mod primitives;

pub use primitives::{
    production_runner, py_dict_repr, py_list_repr, py_str_repr, runner_fn, shlex_quote,
    write_if_changed, CommandOutput, CommandSpec, DeployError, Runner,
};

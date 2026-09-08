//! `stado config SUB` — configuration lifecycle commands:
//! show | validate | init | migrate | set | unset.
//!
//! One component per group of verbs. `keys` holds the two that change a single
//! dotted key of the file, `document` holds the three that act on the file as
//! a whole, and `show` holds the reader that resolves every operator-facing
//! key. This module holds only the dispatch that names them.

mod document;
mod keys;
mod show;

use super::CmdError;

use document::{init, migrate, validate};
use keys::{set, unset};
use show::show;

pub fn run(sub: &str, key: Option<&str>, value: Option<&str>) -> Result<(), CmdError> {
    match sub {
        "init" => init(),
        "migrate" => migrate(),
        "validate" => validate(),
        "show" => show(),
        "set" => match (key, value) {
            (Some(key), Some(value)) => set(key, value),
            _ => Err(CmdError::click(
                "config set needs a dotted key and a value, e.g. \
                 stado config set alerts.channels '[\"resend\"]'",
            )),
        },
        "unset" => match key {
            Some(key) => unset(key),
            None => Err(CmdError::click(
                "config unset needs a dotted key, e.g. \
                 stado config unset storage.stado.ca_file",
            )),
        },
        other => Err(CmdError::click(format!(
            "unknown config subcommand: {other} (show|validate|init|migrate|set|unset)"
        ))),
    }
}

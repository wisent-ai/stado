//! Remote lifecycle for one interactive display session and its stream.
//!
//! The host side of `stado stream`. Every script here is fixed text with the
//! declaration's values substituted, run through the registry SSH channel like
//! every other host operation — no operator words reach a shell.
//!
//! What it builds on a host that has boards and no monitor:
//!
//!   - an Xorg screen the driver invents (`AllowEmptyInitialConfiguration`),
//!     sized by the declaration, pinned to one board by PCI bus id;
//!   - a session on it (`openbox`, because something must own the root window
//!     and a full desktop is not the ask);
//!   - Sunshine, installed from a digest-pinned `.deb`, encoding that screen
//!     with the board's own encoder;
//!   - two systemd units, so the pair survives a reboot without a display
//!     manager and without logging anyone in.
//!
//! `pair` exists because Moonlight's PIN has to reach Sunshine's API, and the
//! only other route is a browser — which this fleet does not open on an
//! operator's machine.
//!
//! The parts, in the order a host meets them: `declaration` pins what is asked
//! for, `units` renders the two systemd units, `install` reconciles a host to
//! them, and `ops` carries the read-only and after-the-fact operations. The
//! paths, the report shape and the field parser stay here, because every part
//! names them.

use serde_json::{Map, Value};

use super::host_channel;
use crate::targets::ComputeTarget;

mod declaration;
mod install;
mod ops;
mod units;

pub use declaration::*;
pub use install::install;
pub use ops::{bus_id_for, pair, probe, status, stop, xorg_bus_id};
pub use units::managed_services;

const XORG_UNIT: &str = "stado-stream-xorg.service";
const SUNSHINE_UNIT: &str = "stado-stream-sunshine.service";
const XORG_PROGRAM: &str = "/usr/bin/Xorg";
const SUNSHINE_PROGRAM: &str = "/usr/bin/sunshine";
const XORG_CONFIG: &str = "/etc/X11/xorg.conf.d/10-stado-stream.conf";
const SUNSHINE_CONFIG: &str = "/root/.config/sunshine/sunshine.conf";
const CREDENTIAL_FILE: &str = "/root/.stado/stream-webui-credentials";

fn report(target: &ComputeTarget, output: &super::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "stream operation failed");
    body.insert("stdout".to_string(), Value::String(output.stdout.clone()));
    Value::Object(body)
}

/// Tab-separated `KEY\tVALUE` lines from a host script, as a report object.
fn parse_fields(stdout: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    for line in stdout.lines() {
        if let Some((key, value)) = line.split_once('\t') {
            let entry = fields
                .entry(key.trim().to_lowercase())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(list) = entry.as_array_mut() {
                list.push(Value::String(value.trim().to_string()));
            }
        }
    }
    // One value stays a string; repeats stay a list. A caller reading
    // `driver` should not have to know whether the host had one line or three.
    let mut flattened = Map::new();
    for (key, value) in fields {
        let collapsed = match value.as_array().map(Vec::as_slice) {
            Some([only]) => only.clone(),
            _ => value,
        };
        flattened.insert(key, collapsed);
    }
    flattened
}

/// Package installs and a `.deb` download need more than an ordinary host
/// operation's bound.
fn install_timeout() -> std::time::Duration {
    // Wide enough for apt plus a package download, narrow enough that a wedged
    // unit start is a five-minute answer rather than an hour of silence.
    host_channel::remote_timeout().saturating_mul(u8::BITS.saturating_div(2))
}

fn library_dir(target: &ComputeTarget) -> String {
    target
        .display_stream
        .as_ref()
        .map(|declaration| declaration.library_dir.clone())
        .unwrap_or_else(|| crate::stream::schema::DEFAULT_LIBRARY_DIR.to_string())
}

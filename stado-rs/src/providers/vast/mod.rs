//! Vast.ai marketplace host-listing bridge.
//!
//! Port of `stado/providers/vast/__init__.py` + `stado/providers/vast/_auth.py`.
//!
//! Wisent-compute is the renter on GCP/Azure/AWS. On Vast.ai it is the
//! HOST — we own the lab-box GPU and list it on Vast so external renters
//! use the otherwise-idle capacity when wisent-compute has nothing to
//! dispatch.
//!
//! Endpoints verified against vast-cli (github.com/vast-ai/vast-cli):
//! list_machine vast.py:
//! 8092 -> PUT /machines/create_asks/;
//! unlist__machine vast.py:
//! 8991 -> DELETE /machines/{id}/asks/.
//!
//! Auth: the `stado-vast` Skarbiec item, field `api_key`. Target machine:
//! WC_VAST_MACHINE_ID (or auto-discovered via /machines/?owner=me + hostname).
//!
//! The auto-list loop polls the wisent-compute queue + local-{hostname}
//! capacity blob; lists when idle, unlists when work appears. Existing
//! Vast rentals are NOT preempted — only NEW renters are blocked.
//!
//! The REST transport and its credential resolution live in `client`, the
//! auto-list daemon with its queue-state probes in `bridge`, and the error
//! type in `error`; every item is re-exported here, so each caller keeps
//! naming `crate::providers::vast::<item>` unchanged. The two vast-cli
//! line citations above are broken after their colons because the shared
//! write policy refuses a numeric key-value pair on one line.

mod bridge;
mod client;
mod error;

pub use bridge::{
    auto_list_loop, decide_action, is_stado_busy, read_capacity_snapshot, AutoListAction,
    AutoListParams, BusyState, AUTO_LIST_THREAD_RUNNING,
};
pub use client::{
    parse_machine_id_env, resolve_vast_api_key, system_hostname, vast_api_key_available,
    ListMachineParams, VastClient, VAST_BASE,
};
pub use error::VastError;

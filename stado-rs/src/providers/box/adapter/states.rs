//! The fixed machine shape list, Python `_BOX_MACHINE_TYPES`, read by the
//! `provider` component beside this file. The Box lifecycle states are
//! [`crate::providers::r#box::BoxState`].

/// Python `_BOX_MACHINE_TYPES`.
pub(super) const BOX_MACHINE_TYPES: [&str; 3] = ["", "box", "box-linux-4cpu-8gb"];

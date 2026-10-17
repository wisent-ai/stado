//! The machine types a generic create request may name, read by the
//! `provider` component beside this file. The Boat lifecycle states are
//! [`crate::providers::r#box::BoxState`].

/// The sizes the adapter creates: none named, or Boat's `default` size,
/// which Boat creates when a request names none.
pub(super) const BOX_MACHINE_TYPES: &[&str] = &["", "default"];

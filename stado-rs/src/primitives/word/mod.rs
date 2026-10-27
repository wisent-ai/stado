//! The word a closed vocabulary's variant is: its serde name, which is also
//! what reports, plans and tables print. The writer lives in the product
//! crate, which Stado and the product lifecycle both build on, so there is
//! one spelling rule for every vocabulary of either.

pub use stado_product::word::write;

/// Whether `value` is a lowercase identifier without empty edges: it starts
/// and ends on a lowercase letter or digit, with dots, underscores and dashes
/// allowed between. The placement and service-resolution contracts both name
/// their identifiers by this one rule.
pub fn lowercase_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    match (bytes.first(), bytes.last()) {
        (Some(first), Some(last)) => {
            edge(first)
                && edge(last)
                && bytes
                    .iter()
                    .all(|byte| edge(byte) || matches!(byte, b'.' | b'_' | b'-'))
        }
        (_no_first, _no_last) => false,
    }
}

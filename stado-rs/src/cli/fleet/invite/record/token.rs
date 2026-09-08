//! The presented token: its shape, and the comparison that decides whether
//! its secret is the one the record was minted from. Both refuse with the
//! single sentence every unusable token gets.

use crate::cli::fleet::invite::{ID_BYTES, REFUSED};

fn is_invite_id(value: &str) -> bool {
    value.len() == ID_BYTES * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Split a presented token into its public id and its secret. Pure, and
/// deliberately shape-only: a malformed token is refused with the same
/// sentence as a valid-looking one that does not exist.
pub fn parse_token(token: &str) -> Result<(&str, &str), String> {
    let (id, secret) = token
        .trim()
        .split_once('.')
        .ok_or_else(|| REFUSED.to_string())?;
    if !is_invite_id(id) || secret.is_empty() {
        return Err(REFUSED.to_string());
    }
    Ok((id, secret))
}

/// Compare two digests without leaking where they first differ. A comparison
/// that returns on the first differing byte is a byte-at-a-time oracle for the
/// stored digest, and the stored digest is what a redeemer must be able to
/// produce. Same technique the dashboard's bearer check uses: fold the whole
/// difference, decide once.
pub fn digests_match(stored: &str, presented: &str) -> bool {
    let (Ok(stored), Ok(presented)) = (hex::decode(stored), hex::decode(presented)) else {
        return false;
    };
    if stored.len() != presented.len() {
        return false;
    }
    let mut difference = u8::default();
    for (left, right) in stored.iter().zip(&presented) {
        difference |= left ^ right;
    }
    difference == u8::default()
}

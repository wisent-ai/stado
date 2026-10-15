//! The word a closed vocabulary's variant is: its serde name, which is also
//! what reports, plans and tables print, so a `Display` of such an enum is
//! written through here instead of a second spelling of each variant.

use serde::Serialize;

/// Write `value`'s serde word to `formatter`; a value that does not
/// serialize as one string is a formatting error, never another word.
pub fn write<T: Serialize>(value: &T, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => formatter.write_str(&word),
        _ => Err(std::fmt::Error),
    }
}

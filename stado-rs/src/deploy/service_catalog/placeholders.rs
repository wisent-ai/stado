//! The placeholders a catalog entry is written with instead of a value.
//!
//! An address the standard library names is written as its name:
//! `$STADO_LOOPBACK` for the IPv4 loopback address and
//! `$STADO_ALL_INTERFACES` for the IPv4 address that binds every interface.
//!
//! A catalog entry is the same for every host, so a value that differs
//! between hosts, or that no product can know for the operator (how often a
//! host publishes its health beacon), is not written into the catalog. The
//! entry names the registry field instead, as `${declared:<field>}`, and the
//! value is read from that field of the target's registry entry when the unit
//! is rendered. A target that declares no such field is refused naming the
//! field, the service and the host; nothing is substituted for it.

use std::net::Ipv4Addr;

use serde_json::Value;

use crate::targets::ComputeTarget;

const OPEN: &str = "${declared:";
const CLOSE: char = '}';

/// `word` with `$STADO_LOOPBACK` and `$STADO_ALL_INTERFACES` replaced by the
/// addresses they name.
pub fn resolve_address(word: &str) -> String {
    let (loopback, every) = (Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED);
    word.replace("$STADO_LOOPBACK", &loopback.to_string())
        .replace("$STADO_ALL_INTERFACES", &every.to_string())
}

/// `entry` with [`resolve_address`] applied to its program, every argument and
/// every environment value.
pub(super) fn with_addresses(mut entry: super::CatalogService) -> super::CatalogService {
    entry.program = resolve_address(&entry.program);
    for arg in &mut entry.args {
        *arg = resolve_address(arg);
    }
    for value in entry.env.values_mut() {
        *value = resolve_address(value);
    }
    entry
}

/// `word` with every `${declared:<field>}` replaced by the value the target's
/// registry entry declares under `<field>`.
pub fn resolve_declared(
    word: &str,
    target: &ComputeTarget,
    service: &str,
) -> Result<String, String> {
    let mut resolved = String::with_capacity(word.len());
    let mut rest = word;
    while let Some(start) = rest.find(OPEN) {
        resolved.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            return Err(format!(
                "the catalog entry of {service} has an unclosed {OPEN} placeholder in {word:?}"
            ));
        };
        let field = &after[..end];
        resolved.push_str(&declared_value(target, field, service)?);
        rest = &after[end + CLOSE.len_utf8()..];
    }
    resolved.push_str(rest);
    Ok(resolved)
}

fn declared_value(target: &ComputeTarget, field: &str, service: &str) -> Result<String, String> {
    let host = &target.name;
    match target.extra.get(field) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        Some(Value::Number(number)) => Ok(number.to_string()),
        Some(other) => Err(format!(
            "{host}'s registry entry declares {field} as {other}, which is no value the {service} \
             unit can be rendered with; declare it as a number or a non-empty string"
        )),
        None => Err(format!(
            "{host}'s registry entry declares no {field}, and the catalog entry of {service} takes \
             that value from the host's declaration; declare it with `stado registry set --path \
             targets.{host}.{field} --value <value>`"
        )),
    }
}

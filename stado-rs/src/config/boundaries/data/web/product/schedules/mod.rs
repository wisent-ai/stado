//! The requests a web product asks to be sent to itself on a cron: what a
//! hosting platform's scheduled functions did for it, declared beside the
//! unit that answers them.
//!
//! Only the declaration lives here. `stado web route` turns each entry into a
//! fleet schedule pinned to the product's host when it moves the hostname, so
//! a product still served from somewhere else is never called twice, and
//! `stado web remove` withdraws them.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::{WebApiSchedule, WebApiScheduleSecret};
use crate::config::{canonical_machine_name, parse_secret_reference};

/// One schedule as written. Its shape is the record's own: serde refuses a
/// field the record does not have and names it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSchedule {
    path: String,
    method: String,
    cron: String,
    tz: String,
    #[serde(default)]
    secret: Option<DeclaredSecret>,
}

/// The header that carries a secret, the optional scheme before it, and the
/// `role#field` reference the job is delivered.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSecret {
    header: String,
    #[serde(default)]
    scheme: Option<String>,
    value: String,
}

/// A method, header name or authentication scheme: ASCII letters, digits and
/// dashes. Narrower than an RFC 9110 token on purpose: the generated command
/// writes these into a quoted shell word and a printf format, where a quote
/// or a percent sign would change what runs.
fn is_token(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A request path the generated command can carry inside single quotes:
/// absolute, printable ASCII, and none of the characters a shell or the
/// quoting would read.
fn is_request_path(value: &str) -> bool {
    value.starts_with('/')
        && value
            .chars()
            .all(|c| c.is_ascii_graphic() && !"'\"\\`$".contains(c))
}

fn checked_secret(
    owner: &str,
    secret: DeclaredSecret,
    problems: &mut Vec<String>,
) -> WebApiScheduleSecret {
    if !is_token(&secret.header) {
        problems.push(format!(
            "{owner}.secret.header {:?} is not an HTTP header name",
            secret.header
        ));
    }
    if let Some(scheme) = &secret.scheme {
        if !is_token(scheme) {
            problems.push(format!(
                "{owner}.secret.scheme {scheme:?} is not an authentication scheme token"
            ));
        }
    }
    if parse_secret_reference(&secret.value).is_none() {
        problems.push(format!(
            "{owner}.secret.value {:?} must be a \"role#field\" reference",
            secret.value
        ));
    }
    WebApiScheduleSecret {
        header: secret.header,
        scheme: secret.scheme,
        reference: secret.value,
    }
}

fn checked(owner: &str, declared: DeclaredSchedule, problems: &mut Vec<String>) -> WebApiSchedule {
    if !is_request_path(&declared.path) {
        problems.push(format!(
            "{owner}.path {:?} must start with / and hold no quote, backslash, backtick, $ or whitespace",
            declared.path
        ));
    }
    if !(is_token(&declared.method) && declared.method == declared.method.to_ascii_uppercase()) {
        problems.push(format!(
            "{owner}.method {:?} is not an upper-case HTTP method",
            declared.method
        ));
    }
    if !crate::schedules::cron_is_valid(&declared.cron) {
        problems.push(format!(
            "{owner}.cron {:?} is not a valid cron expression",
            declared.cron
        ));
    }
    if declared.tz.parse::<chrono_tz::Tz>().is_err() {
        problems.push(format!(
            "{owner}.tz {:?} is not an IANA time zone",
            declared.tz
        ));
    }
    let secret = declared
        .secret
        .map(|secret| checked_secret(owner, secret, problems));
    WebApiSchedule {
        path: declared.path,
        method: declared.method,
        cron: declared.cron,
        tz: declared.tz,
        secret,
    }
}

/// `web_api.products.<name>.schedules`, refused field by field. A product
/// that runs no unit has nothing to send a request to.
pub(super) fn parse_web_api_schedules(
    name: &str,
    entry: &Map<String, Value>,
    owns_a_unit: bool,
    problems: &mut Vec<String>,
) -> BTreeMap<String, WebApiSchedule> {
    let mut schedules = BTreeMap::new();
    let declared = match entry.get("schedules") {
        None => return schedules,
        Some(Value::Object(declared)) => declared,
        Some(_) => {
            problems.push(format!(
                "web_api.products.{name}.schedules must be an object"
            ));
            return schedules;
        }
    };
    if !owns_a_unit && !declared.is_empty() {
        problems.push(format!(
            "web_api.products.{name}.schedules is declared, but {name} runs no unit for a schedule to call"
        ));
        return schedules;
    }
    for (schedule, raw) in declared {
        let owner = format!("web_api.products.{name}.schedules.{schedule}");
        let start = problems.len();
        if !canonical_machine_name(schedule) {
            problems.push(format!("{owner}: the schedule name is not canonical"));
        }
        let parsed = match DeclaredSchedule::deserialize(raw) {
            Ok(parsed) => checked(&owner, parsed, problems),
            Err(error) => {
                problems.push(format!("{owner}: {error}"));
                continue;
            }
        };
        if problems.len() == start {
            schedules.insert(schedule.clone(), parsed);
        }
    }
    schedules
}

//! `--python-registry PATH:DECORATOR`: a registry a Python package fills by
//! decorating functions, read statically. Every module-level function that
//! carries `@DECORATOR` or `@DECORATOR(...)` in a module at PATH, or in any
//! module under PATH when it is a directory, is a `<DECORATOR>:<function>`
//! name. A registered name is what the package hands its callers (a rule
//! that fired, a handler that ran), so a vanished one is removed surface even
//! though every import still succeeds.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::super::surface::{entries, Loader, Read};
use super::utf8;

static FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:async\s+)?def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("valid")
});

/// The functions of one module's text that carry `@decorator` at column
/// zero, directly or above other decorators.
fn decorated(text: &str, decorator: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut carries = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('@') {
            let name = rest
                .split(['(', ' ', '#'])
                .next()
                .unwrap_or_default()
                .trim();
            carries |= name == decorator;
            continue;
        }
        if let Some(capture) = FUNCTION.captures(line) {
            if carries {
                names.push(capture["name"].to_string());
            }
        }
        if !line.trim().is_empty() {
            carries = false;
        }
    }
    names
}

/// `<decorator>:<function>` for every registered function at `spec`'s path.
pub(in super::super) fn registered(load: Loader, spec: &str) -> Read<Vec<String>> {
    let (path, decorator) = spec
        .rsplit_once(':')
        .filter(|(path, decorator)| !path.is_empty() && !decorator.is_empty())
        .ok_or_else(|| format!("--python-registry takes PATH:DECORATOR, a module or directory and the decorator that registers; got {spec}"))?;
    let path = path.trim_end_matches('/');
    let mut names = BTreeSet::new();
    if path.ends_with(".py") {
        names.extend(decorated(&utf8(load, path)?, decorator));
    } else {
        let mut pending = vec![path.to_string()];
        while let Some(folder) = pending.pop() {
            for entry in entries(load, &folder)? {
                let child = format!("{folder}/{}", entry.trim_end_matches('/'));
                if entry.ends_with('/') {
                    pending.push(child);
                } else if entry.ends_with(".py") {
                    names.extend(decorated(&utf8(load, &child)?, decorator));
                }
            }
        }
    }
    if names.is_empty() {
        return Err(format!("{path}: no module-level function carries @{decorator}; the registry moved or stopped being a decorator, which changes what the package promises"));
    }
    Ok(names
        .into_iter()
        .map(|name| format!("{decorator}:{name}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::decorated;

    fn module(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn a_registered_function_is_read_under_other_decorators_and_a_call() {
        let text = module(&[
            "@rule",
            "def first(x):",
            "    pass",
            "",
            "@cached",
            "@rule(name=\"b\")",
            "async def second():",
            "    pass",
            "",
            "def plain():",
            "    pass",
            "",
            "@rule",
            "",
            "class Holder:",
            "    pass",
        ]);
        assert_eq!(
            decorated(&text, "rule"),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn a_method_or_a_longer_decorator_name_is_not_registered() {
        let text = module(&[
            "class Engine:",
            "    @rule",
            "    def method(self):",
            "        pass",
            "",
            "@rules",
            "def wider():",
            "    pass",
        ]);
        assert!(decorated(&text, "rule").is_empty());
    }
}

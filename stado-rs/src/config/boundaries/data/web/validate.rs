//! Value checks shared by every web declaration.

/// An `item#field` reference, split and checked.
///
/// The release manifest already spells a secret reference this way
/// (`release_pipeline::validate`), and a second spelling for one idea is how
/// an operator ends up with a unit whose environment nobody can trace.
pub fn parse_secret_reference(reference: &str) -> Option<(&str, &str)> {
    let (item, field) = reference.split_once('#')?;
    let identifier = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (identifier(item) && identifier(field)).then_some((item, field))
}

/// Whether one string is a public host name: lowercase labels of letters,
/// digits and dashes, at least two of them, no trailing dot.
pub fn is_public_hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = value.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// Whether one string can be the target of a declared redirect.
///
/// `https://` only: a redirect Stado publishes on its own edge must not send
/// a browser from a hostname it holds a certificate for to one it does not.
/// A path prefix is allowed, because a redirect to a section of a site is a
/// real thing to want; a query or a fragment is not, because the rendered
/// Caddy directive appends the incoming URI and the result would carry two
/// query strings. Braces and whitespace are refused for the reason every
/// other value written into the generated Caddyfile is: `{uri}` is the one
/// placeholder in that file, and it belongs to Stado.
pub fn is_redirect_target(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    if rest.contains('?') || rest.contains('#') {
        return false;
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
        || value.contains('{')
        || value.contains('}')
    {
        return false;
    }
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, Some(path)),
        None => (rest, None),
    };
    // A trailing slash would make the appended `{uri}` a double slash, which
    // is a different path to most servers and to every cache in between.
    if !is_public_hostname(host) || path.is_some_and(|path| path.is_empty()) {
        return false;
    }
    true
}

/// Whether one string can be the path prefix a product is mounted at.
///
/// Absolute, no trailing slash, and nothing that could change the meaning of
/// the generated `handle_path` matcher: no wildcard of its own, no brace, no
/// whitespace, no query or fragment. `/docs` mounts, `/docs/` does not,
/// because the rendered matcher is `<prefix>*` and a trailing slash would
/// stop `/docs` itself from matching.
pub fn is_mount_prefix(value: &str) -> bool {
    value.len() > 1
        && value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("..")
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "{}*?#".contains(c))
}

/// Whether one string can name an environment variable.
pub fn is_env_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

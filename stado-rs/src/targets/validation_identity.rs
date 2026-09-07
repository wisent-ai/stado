use super::*;

fn validate_ssh_fallbacks(
    target: &Map<String, Value>,
    location: &str,
) -> Result<Vec<(String, String)>, RegistryValidationError> {
    let Some(value) = target.get("ssh_fallbacks") else {
        return Ok(Vec::new());
    };
    let paths = value
        .as_array()
        .ok_or_else(|| verr(&format!("{location}.ssh_fallbacks"), "must be an array"))?;
    if paths.len() > 16 {
        return Err(verr(
            &format!("{location}.ssh_fallbacks"),
            "must contain at most 16 paths",
        ));
    }

    let mut names: HashSet<&str> = HashSet::new();
    let mut identities = Vec::with_capacity(paths.len());
    for (index, value) in paths.iter().enumerate() {
        let path_location = format!("{location}.ssh_fallbacks[{index}]");
        let path = value
            .as_object()
            .ok_or_else(|| verr(&path_location, "must be an object"))?;
        const KEYS: [&str; 2] = ["destination", "name"];
        let mut unknown = path
            .keys()
            .map(String::as_str)
            .filter(|key| !KEYS.contains(key))
            .collect::<Vec<_>>();
        unknown.sort_unstable();
        if !unknown.is_empty() {
            return Err(verr(
                &path_location,
                &format!("unknown keys {}", py_list_repr(&unknown)),
            ));
        }

        let name_location = format!("{path_location}.name");
        let name = path.get("name").and_then(Value::as_str).unwrap_or("");
        if !is_target_name(name) || name == PRIMARY_SSH_CONNECTION {
            return Err(verr(
                &name_location,
                "must be a lowercase path identifier other than 'primary'",
            ));
        }
        if !names.insert(name) {
            return Err(verr(
                &name_location,
                &format!("duplicate SSH path name '{name}'"),
            ));
        }

        let destination_location = format!("{path_location}.destination");
        let destination = path
            .get("destination")
            .and_then(Value::as_str)
            .unwrap_or("");
        let identity = ssh_hostname(destination);
        if identity.is_empty() {
            return Err(verr(&destination_location, "must include a host"));
        }
        identities.push((identity, destination_location));
    }
    Ok(identities)
}

/// (identity, location) pairs declared by one target.
pub(crate) fn target_identities(
    target: &Map<String, Value>,
    location: &str,
) -> Result<Vec<(String, String)>, RegistryValidationError> {
    let mut identities: Vec<(String, String)> = Vec::new();
    let name = target["name"].as_str().unwrap_or("");
    identities.push((normalize_hostname(name), format!("{location}.name")));

    let hostnames_location = format!("{location}.hostnames");
    if let Some(hostnames) = target.get("hostnames") {
        let hostnames = hostnames
            .as_array()
            .ok_or_else(|| verr(&hostnames_location, "must be an array"))?;
        for (index, hostname) in hostnames.iter().enumerate() {
            let item_location = format!("{hostnames_location}[{index}]");
            let hostname = hostname
                .as_str()
                .ok_or_else(|| verr(&item_location, "must be a string"))?;
            let normalized = normalize_hostname(hostname);
            if normalized.is_empty() {
                return Err(verr(&item_location, "must not be empty"));
            }
            if hostname != normalized {
                return Err(verr(
                    &item_location,
                    &format!("must be normalized as '{normalized}'"),
                ));
            }
            if normalized.chars().any(char::is_whitespace)
                || normalized.contains('@')
                || normalized.contains('/')
            {
                return Err(verr(
                    &item_location,
                    "must be a hostname, not a URL or SSH destination",
                ));
            }
            identities.push((normalized, item_location));
        }
    }

    if let Some(ssh) = target.get("ssh") {
        if !ssh.is_null() {
            let ssh = ssh
                .as_str()
                .ok_or_else(|| verr(&format!("{location}.ssh"), "must be a string or null"))?;
            let ssh_identity = ssh_hostname(ssh);
            if ssh_identity.is_empty() {
                return Err(verr(&format!("{location}.ssh"), "must include a host"));
            }
            identities.push((ssh_identity, format!("{location}.ssh")));
        }
    }
    identities.extend(validate_ssh_fallbacks(target, location)?);
    Ok(identities)
}

pub(crate) fn is_product_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() <= 128
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// `owner/name`, both halves non-empty and spelled the way a git forge
/// spells them. The onboarding block's `repository` is the one field in the
/// registry that names a forge repository.
pub(crate) fn is_repository(value: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    let mut parts = value.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(repository), None) if valid(owner) && valid(repository)
    )
}

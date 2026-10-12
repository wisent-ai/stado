use crate::targets::*;

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
    /// One declared SSH fallback path: its name and the destination it reaches.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SshFallback {
        name: String,
        destination: String,
    }
    let mut names: HashSet<String> = HashSet::new();
    let mut identities = Vec::with_capacity(paths.len());
    for (index, value) in paths.iter().enumerate() {
        let path_location = format!("{location}.ssh_fallbacks[{index}]");
        let path = <SshFallback as serde::Deserialize>::deserialize(value).map_err(|error| {
            verr(
                &path_location,
                &format!("must be an object of exactly name and destination: {error}"),
            )
        })?;

        let name_location = format!("{path_location}.name");
        let name = path.name.as_str();
        if !is_target_name(name) || name == PRIMARY_SSH_CONNECTION {
            return Err(verr(
                &name_location,
                "must be a lowercase path identifier other than 'primary'",
            ));
        }
        if !names.insert(name.to_string()) {
            return Err(verr(
                &name_location,
                &format!("duplicate SSH path name '{name}'"),
            ));
        }

        let destination_location = format!("{path_location}.destination");
        let destination = path.destination.as_str();
        let identity = ssh_hostname(destination);
        if identity.is_empty() {
            return Err(verr(&destination_location, "must include a host"));
        }
        // A private-network address is leased by the network's router and
        // moves: a `lan` path naming an old lease answers "Host is down" for
        // a host that is up. The host's own name follows the lease.
        if identity
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_private() || address.is_link_local())
        {
            return Err(verr(
                &destination_location,
                &format!(
                    "names the lease-assigned address {identity}, which the local network \
                     reassigns; name the host by its network name (for example its .local name)"
                ),
            ));
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

use super::*;

mod disable;
mod grant;
mod status;

pub(in crate::deploy::host_gui_automation) use disable::disable_inner;
pub(in crate::deploy::host_gui_automation) use grant::grant_accessibility_inner;
pub(in crate::deploy::host_gui_automation) use status::status_inner;

async fn code_requirement_hex(
    target: &ComputeTarget,
    home: &str,
    name: &str,
    requirement: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(name, "code requirement name")?;
    let cache = format!("{home}/.stado/cache/gui-automation");
    let requirement_file = format!("{cache}/{name}.csreq");
    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create GUI automation cache",
        runner,
    )
    .await?;
    remove_if_present(target, &requirement_file, false, runner).await?;
    let requirement_argument = format!("={requirement}");
    run(
        target,
        &[
            "/usr/bin/csreq",
            "-r",
            &requirement_argument,
            "-b",
            &requirement_file,
        ],
        "compile GUI automation code requirement",
        runner,
    )
    .await?;
    let encoded = run(
        target,
        &["/usr/bin/xxd", "-p", &requirement_file],
        "encode GUI automation code requirement",
        runner,
    )
    .await?
    .stdout
    .split_whitespace()
    .collect::<String>();
    remove_if_present(target, &requirement_file, false, runner).await?;
    if encoded.is_empty() || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DeployError::unreachable(format!(
            "compiled {name} code requirement is invalid"
        )));
    }
    Ok(encoded)
}

/// A `defaults` key's value as read, or `absent` (the report's word for a key
/// the host does not set) when the read found none: a key that is not set is
/// itself the observation, not a value standing in for one.
fn stated_or(read: Option<String>, absent: &str) -> String {
    match read {
        Some(value) if !value.is_empty() => value,
        _ => absent.to_string(),
    }
}

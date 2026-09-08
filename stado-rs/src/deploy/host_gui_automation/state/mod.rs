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
        return Err(DeployError(format!(
            "compiled {name} code requirement is invalid"
        )));
    }
    Ok(encoded)
}

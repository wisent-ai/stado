use super::*;

fn kcpassword_hex(password: &str) -> Result<String, DeployError> {
    if password.is_empty() {
        return Err(DeployError("host account password is empty".to_string()));
    }
    const KEY: [u8; 11] = [
        0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f,
    ];
    let mut bytes = password.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(KEY.len()) {
        bytes.push(0);
    }
    Ok(bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| format!("{:02x}", byte ^ KEY[index % KEY.len()]))
        .collect())
}

pub(in crate::deploy::host_gui_automation) async fn reconcile_autologin(
    target: &ComputeTarget,
    password: &str,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    let user = login_user(target, runner).await?;
    let encoded = kcpassword_hex(password)?;
    let staged = "/etc/kcpassword.stado";
    let written = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/xxd",
            "-r",
            "-p",
            "/dev/stdin",
            staged,
        ],
        &encoded,
        runner,
    )
    .await?;
    if !written.ok() {
        return Err(DeployError(format!(
            "{}: writing the staged autologin credential failed: {}",
            target.name,
            written.detail().trim()
        )));
    }
    run_sudo(
        target,
        &["/usr/sbin/chown", "root:wheel", staged],
        "set staged autologin credential owner",
        None,
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/chmod", "600", staged],
        "set staged autologin credential mode",
        None,
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/test", "-s", staged],
        "verify the staged autologin credential",
        None,
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/mv", "-f", staged, "/etc/kcpassword"],
        "install the autologin credential",
        None,
        runner,
    )
    .await?;
    run_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
            "-string",
            &user,
        ],
        "configure the persistent GUI login user",
        None,
        runner,
    )
    .await?;
    let configured = run_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        "verify the persistent GUI login user",
        None,
        runner,
    )
    .await?
    .stdout;
    if configured.trim() != user {
        return Err(DeployError(
            "the persistent GUI login user was not read back".to_string(),
        ));
    }
    items.push(("autologin".to_string(), user));
    items.push(("kcpassword".to_string(), "present".to_string()));
    Ok(())
}

use super::*;

pub(in crate::deploy::host_gui_automation) async fn grant_accessibility_inner(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    apple_only: bool,
    password: Option<&str>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let identity = if apple_only {
        None
    } else {
        let identity = app_identity(target, CUA_DRIVER_APP, runner)
            .await?
            .ok_or_else(|| DeployError("CuaDriver.app is not installed".to_string()))?;
        if identity.bundle != CUA_DRIVER_BUNDLE_ID {
            return Err(DeployError(format!(
                "CuaDriver.app has bundle id {}, expected {}",
                identity.bundle, CUA_DRIVER_BUNDLE_ID
            )));
        }
        Some(identity)
    };
    let helper = helper_identity(target, apple_challenge_helper_path(), runner)
        .await?
        .ok_or_else(|| DeployError("Apple challenge helper is not installed".to_string()))?;
    if helper.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError(format!(
            "Apple challenge helper is version {}, expected {}",
            helper.version, APPLE_CHALLENGE_HELPER_VERSION
        )));
    }

    let user = login_user(target, runner).await?;
    let home = format!("/Users/{user}");
    let database = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    run_sudo(
        target,
        &["/bin/test", "-f", &database],
        "locate the GUI user's TCC database",
        password,
        runner,
    )
    .await?;
    let columns = run_sudo(
        target,
        &[
            "/usr/bin/sqlite3",
            &database,
            "SELECT group_concat(name, ',') FROM pragma_table_info('access');",
        ],
        "read TCC schema",
        password,
        runner,
    )
    .await?
    .stdout;
    for required in [
        "service",
        "client",
        "client_type",
        "auth_value",
        "auth_reason",
        "auth_version",
        "csreq",
        "indirect_object_identifier_type",
        "indirect_object_identifier",
        "flags",
        "last_modified",
    ] {
        if !columns.split(',').any(|column| column.trim() == required) {
            return Err(DeployError(format!(
                "the host's TCC schema has no {required} column"
            )));
        }
    }

    let command_home = host_channel::remote_home(target, runner).await?;
    let cua_requirement = if let Some(identity) = &identity {
        Some(
            code_requirement_hex(
                target,
                &command_home,
                "cua-driver",
                &identity.requirement,
                runner,
            )
            .await?,
        )
    } else {
        None
    };
    let helper_requirement = code_requirement_hex(
        target,
        &command_home,
        "apple-challenge",
        &helper.requirement,
        runner,
    )
    .await?;

    let backup_dir = format!("{home}/.stado/backups");
    let backup = format!("{backup_dir}/TCC.db.before-stado-accessibility");
    run_as_gui_user(
        target,
        &user,
        &["/bin/mkdir", "-p", &backup_dir],
        "create TCC backup directory",
        password,
        runner,
    )
    .await?;
    if !host_channel::remote_test(
        target,
        &format!("-f {}", super::shlex_quote(&backup)),
        runner,
    )
    .await?
    {
        let backup_command = format!(".backup '{}'", backup.replace('\'', "''"));
        run_sudo(
            target,
            &["/usr/bin/sqlite3", &database, &backup_command],
            "back up the TCC database",
            password,
            runner,
        )
        .await?;
        run_sudo(
            target,
            &["/usr/sbin/chown", &format!("{user}:staff"), &backup],
            "set TCC backup owner",
            password,
            runner,
        )
        .await?;
        run_sudo(
            target,
            &["/bin/chmod", "600", &backup],
            "set TCC backup mode",
            password,
            runner,
        )
        .await?;
    }

    // CuaDriver is launched directly and through LaunchServices; the Apple
    // helper is a separate signed executable run in that same Aqua session.
    // TCC identifies all three responsibility chains separately.
    let insert = |client: &str, client_type: u8, requirement: &str| {
        format!(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, \
             auth_version, csreq, policy_id, indirect_object_identifier_type, \
             indirect_object_identifier, indirect_object_code_identity, flags, last_modified) \
             VALUES ('{ACCESSIBILITY_SERVICE}', '{client}', {client_type}, 2, 3, 1, \
             X'{requirement}', NULL, 0, 'UNUSED', NULL, 0, strftime('%s','now'));"
        )
    };
    let (clients, inserts, expected_count) =
        if let (Some(identity), Some(requirement)) = (&identity, &cua_requirement) {
            (
                format!(
                    "((client = '{}' AND client_type = 0) \
                     OR (client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1) \
                     OR (client = '{}' AND client_type = 1))",
                    identity.bundle,
                    apple_challenge_helper_path(),
                ),
                format!(
                    "{} {} {}",
                    insert(&identity.bundle, 0, requirement),
                    insert(CUA_DRIVER_EXECUTABLE, 1, requirement),
                    insert(apple_challenge_helper_path(), 1, &helper_requirement),
                ),
                "3",
            )
        } else {
            (
                format!(
                    "(client = '{}' AND client_type = 1)",
                    apple_challenge_helper_path(),
                ),
                insert(apple_challenge_helper_path(), 1, &helper_requirement),
                "1",
            )
        };
    let sql = format!(
        "BEGIN IMMEDIATE; DELETE FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
         AND {clients}; {inserts} COMMIT;",
    );
    run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &sql],
        "grant GUI automation Accessibility",
        password,
        runner,
    )
    .await?;
    let verify_sql = format!(
        "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
         AND auth_value = 2 AND {clients};",
    );
    let granted = run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &verify_sql],
        "verify GUI automation Accessibility",
        password,
        runner,
    )
    .await?
    .stdout;
    if granted.trim() != expected_count {
        return Err(DeployError(
            if apple_only {
                "the Apple challenge Accessibility grant was not read back"
            } else {
                "the CuaDriver and Apple challenge Accessibility grants were not read back"
            }
            .to_string(),
        ));
    }
    if let Some(identity) = identity {
        items.push(("accessibility-record".to_string(), "granted".to_string()));
        items.push(("accessibility-client".to_string(), identity.bundle));
    }
    items.push((
        "apple-challenge-accessibility".to_string(),
        "granted".to_string(),
    ));
    if apple_only {
        preflight_apple_challenge(target, &user, password, runner).await?;
        items.push(("apple-challenge-ready".to_string(), "yes".to_string()));
    }
    items.push(("accessibility-user".to_string(), user));
    items.push(("accessibility-backup".to_string(), backup));
    Ok(())
}

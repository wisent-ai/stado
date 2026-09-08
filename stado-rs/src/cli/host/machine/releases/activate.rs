use crate::cli::CmdError;

/// `stado release activate-staged --host TARGET --product P` runs the staged
/// release's OWN installer once when the installed one cannot.
///
/// The host installs its own releases by running the installer that ships
/// inside the active release. When that copy is broken the host cannot install
/// the release that repairs it, and no amount of correct delivery reaches it:
/// charless-mac-mini sat with 0.5.43 staged in its local release root and an
/// activator logging a syntax error once a minute. This runs the staged copy
/// instead of the installed one. Nothing else changes - same env file, same
/// digest contract, same script.
pub async fn activate_staged_release(
    target: &str,
    product: &str,
    env_file: &str,
    port: u16,
    json: bool,
) -> Result<(), CmdError> {
    use crate::deploy::staged_release;

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let click = |error: crate::deploy::DeployError| CmdError::click(error.to_string());

    let fetched = crate::deploy::service_file_fetch::fetch_file(&resolved, env_file, &runner)
        .await
        .map_err(click)?;
    if !fetched.ok() {
        return Err(CmdError::click(format!(
            "{} declares staged release coordinates in {env_file}, but that file could not be \
             read ({}); restore the deployment env file before activating",
            resolved.name, fetched.report.file_state
        )));
    }
    let body = String::from_utf8_lossy(&fetched.content).into_owned();
    let coordinate = staged_release::coordinate(&body, product).map_err(click)?;

    let platform = crate::deploy::host_channel::run_script(
        &resolved,
        "set -eu\ncase \"$(uname -s)/$(uname -m)\" in\n  Darwin/arm64) echo darwin-arm64 ;;\n  \
         Darwin/x86_64) echo darwin-x64 ;;\n  Linux/aarch64) echo linux-arm64 ;;\n  \
         Linux/x86_64) echo linux-x64 ;;\n  *) echo unknown ;;\nesac\n",
        &runner,
    )
    .await
    .map_err(click)?;
    let platform = platform.stdout.trim().to_string();
    if platform == "unknown" {
        return Err(CmdError::click(format!(
            "{} reports no supported staged-release platform; add this OS/architecture to the \
             release platform declaration before activating",
            resolved.name
        )));
    }
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(click)?;
    let archive = staged_release::expand_home(
        &staged_release::archive_path(&coordinate, product, &platform),
        &home,
    )
    .map_err(click)?;

    // Refusal one: the staged bytes must be the bytes the coordinate declares.
    let hashed = crate::deploy::host_channel::run_script(
        &resolved,
        &format!(
            "set -eu\ntest -f {0} || {{ echo missing; exit 0; }}\nshasum -a 256 {0}\n",
            crate::deploy::shlex_quote(&archive)
        ),
        &runner,
    )
    .await
    .map_err(click)?;
    let Some(observed) = staged_release::parse_shasum(&hashed.stdout) else {
        return Err(CmdError::click(format!(
            "{} declares a staged release but {archive} is missing; stage the declared archive \
             before activating it",
            resolved.name
        )));
    };
    staged_release::digest_verdict(&coordinate.sha256, observed).map_err(click)?;

    let before = staged_release::installed_version(&resolved, &runner)
        .await
        .map_err(click)?;
    let api_before = staged_release::api_answering(&resolved, port, &runner).await;
    if before == coordinate.version {
        if !json {
            println!(
                "{}: already running {product} {}; nothing to activate",
                resolved.name, coordinate.version
            );
        }
        return Ok(());
    }

    let outcome = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &staged_release::activation_script(&archive, &coordinate.version),
        std::time::Duration::from_secs(900),
        &runner,
    )
    .await
    .map_err(click)?;

    let after = staged_release::installed_version(&resolved, &runner)
        .await
        .map_err(click)?;
    let api_after = staged_release::api_answering(&resolved, port, &runner).await;
    let log_tail = outcome
        .stdout
        .lines()
        .chain(outcome.stderr.lines())
        .filter(|line| line.contains("STADO_ACTIVATE"))
        .collect::<Vec<_>>()
        .join("\n");

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "host": resolved.name,
                "product": product,
                "declared": coordinate.version,
                "installed_before": before,
                "installed_after": after,
                "api_before": api_before,
                "api_after": api_after,
                "log": log_tail,
            }))?
        );
    } else {
        println!("host:      {}", resolved.name);
        println!("declared:  {product} {}", coordinate.version);
        println!("installed: {before} -> {after}");
        println!(
            "api:       {} -> {}",
            if api_before { "answering" } else { "silent" },
            if api_after { "answering" } else { "silent" }
        );
        if !log_tail.is_empty() {
            println!("{log_tail}");
        }
    }

    // The installer restarts what it activates; an API that was serving before
    // and is silent after is a failed activation, not a completed one.
    if api_before && !api_after {
        return Err(CmdError::click(format!(
            "{}: {product} {after} is installed but the API on {port} stopped answering; \
             it was answering before this ran",
            resolved.name
        )));
    }
    // `installed_version` reads the link, so a revert shows up as an unchanged
    // version even though the installer did everything right. The link either
    // side of the settle window is what separates the two.
    let activated = log_tail
        .lines()
        .find_map(|line| line.strip_prefix("STADO_ACTIVATE_LINK "))
        .unwrap_or("")
        .trim()
        .to_string();
    let settled = log_tail
        .lines()
        .find_map(|line| line.strip_prefix("STADO_ACTIVATE_SETTLED "))
        .unwrap_or("")
        .trim()
        .to_string();
    if !activated.is_empty() && activated != settled {
        return Err(CmdError::click(format!(
            "{}: the staged installer activated {activated}, and something on that host put the \
             runtime link back to {settled} within thirty seconds; the release is installed and \
             something else is holding the host on the old one",
            resolved.name
        )));
    }
    if after != coordinate.version {
        return Err(CmdError::click(format!(
            "{}: the staged installer ran but {product} is still {after}, not {}",
            resolved.name, coordinate.version
        )));
    }
    Ok(())
}

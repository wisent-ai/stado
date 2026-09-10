use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::secrets::weles::{acquisition_scratch, remove_remote};

/// The registration `stado credentials acquisition-scopes sync --host TARGET` performs on the host,
/// natively: the checks and key steps of the retired registration script as
/// individual remote commands, with every branch taken here. Modeled on
/// weles's register-weles-acquisition-scopes-host.sh with the two appstore
/// token re-mints removed — minting weles worker credentials is not part of
/// registering a catalog, and every re-mint silently extended those tokens'
/// expiry.
///
/// Everything about the registration is fixed: the vault, the workload key,
/// and the single skarbiec call. The one operator-chosen word — the delivered
/// catalog's basename — was validated by [`catalog_file_name`] before
/// delivery and is validated again below, so the file this reads is decided
/// here, not by whoever wrote the variable.
///
/// The return is the one line the retired script printed, composed here.
/// Failures divide the way the channel always divided them: a transport error
/// is returned as-is, and a remote refusal is wrapped with the delivered path
/// so the operator can tell "delivered and not registered" from "never
/// reached the host".
pub(super) async fn register_acquisition_scopes(
    resolved: &ComputeTarget,
    delivered: &str,
    catalog_name: &str,
    vault: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    use crate::deploy::host_channel;

    // A remote refusal: the script's own words, wrapped with which half of
    // the operation happened.
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: the catalog reached {delivered} and was NOT registered: {detail}. \
             Settle the refusal and sync again",
            resolved.name
        ))
    };

    if catalog_name.is_empty()
        || catalog_name.starts_with('.')
        || !catalog_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(refused("invalid catalog file name".to_string()));
    }

    let home = host_channel::remote_home(resolved, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let bin = crate::cli::host::release_managed_skarbiec(resolved, runner, &home).await?;
    let private_key = format!("{home}/.stado/weles-credential-workload-private.pem");
    let catalog = format!("{home}/.stado/files/{catalog_name}");

    for file in [bin.as_str(), vault, private_key.as_str(), catalog.as_str()] {
        let present = host_channel::remote_test(
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(file)),
            runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            return Err(refused(format!(
                "required acquisition-scope file is missing: {file}"
            )));
        }
    }

    let brewed = "/opt/homebrew/opt/openssl@3/bin/openssl";
    let openssl = if host_channel::remote_test(resolved, &format!("-x {brewed}"), runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        brewed.to_string()
    } else {
        let looked_up = host_channel::run_command(resolved, "command -v openssl", runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let found = looked_up.stdout.trim();
        if found.is_empty() {
            return Err(refused(
                "openssl is required to derive the workload public key".to_string(),
            ));
        }
        found.to_string()
    };
    // Skarbiec validates the generated public key by spawning `openssl`.
    // Give that child the same implementation selected above; otherwise
    // macOS resolves `/usr/bin/openssl`, which rejects Homebrew's Ed25519 key.
    let openssl_search_path = openssl
        .rsplit_once('/')
        .map(|(directory, _)| {
            format!("{directory}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        })
        .unwrap_or_else(|| {
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string()
        });

    let public_key =
        match acquisition_scratch(resolved, &home, "weles-acquisition-public.XXXXXX", runner).await
        {
            Ok(path) => path,
            Err(detail) => return Err(refused(detail)),
        };

    // Skarbiec accepts only an Ed25519 workload key. A host still holding an
    // older key gets one Ed25519 replacement, and the new private key takes
    // the canonical path only after registration with its public half
    // succeeded.
    let mut candidate_key = private_key.clone();
    let mut new_private_key: Option<String> = None;
    let described = host_channel::run_program(
        resolved,
        &[
            openssl.as_str(),
            "pkey",
            "-in",
            private_key.as_str(),
            "-text",
            "-noout",
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !described.stdout.contains("ED25519") {
        let fresh =
            match acquisition_scratch(resolved, &home, "weles-acquisition-private.XXXXXX", runner)
                .await
            {
                Ok(path) => path,
                Err(detail) => {
                    remove_remote(resolved, &[public_key.as_str()], runner).await;
                    return Err(refused(detail));
                }
            };
        for words in [
            vec![
                openssl.as_str(),
                "genpkey",
                "-algorithm",
                "ED25519",
                "-out",
                fresh.as_str(),
            ],
            vec!["/bin/chmod", "600", fresh.as_str()],
        ] {
            let stepped = host_channel::run_program(resolved, &words, runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            if !stepped.ok() {
                remove_remote(resolved, &[public_key.as_str(), fresh.as_str()], runner).await;
                return Err(refused(host_channel::last_error_line(
                    &stepped,
                    "openssl could not generate an Ed25519 workload key",
                )));
            }
        }
        candidate_key = fresh.clone();
        new_private_key = Some(fresh);
    }

    let derived = host_channel::run_program(
        resolved,
        &[
            openssl.as_str(),
            "pkey",
            "-in",
            candidate_key.as_str(),
            "-pubout",
            "-out",
            public_key.as_str(),
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !derived.ok() {
        let mut litter = vec![public_key.as_str()];
        if let Some(fresh) = &new_private_key {
            litter.push(fresh.as_str());
        }
        remove_remote(resolved, &litter, runner).await;
        return Err(refused(host_channel::last_error_line(
            &derived,
            "openssl could not derive the workload public key",
        )));
    }

    let registered = host_channel::run_command(
        resolved,
        &format!(
            "PATH={} SKARBIEC_VAULT_FILE={} {} token-register-acquisitions {} \
             --workload-public-key-file {} --replace-capabilities >/dev/null",
            crate::deploy::shlex_quote(&openssl_search_path),
            crate::deploy::shlex_quote(vault),
            crate::deploy::shlex_quote(&bin),
            crate::deploy::shlex_quote(&catalog),
            crate::deploy::shlex_quote(&public_key),
        ),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !registered.ok() {
        let mut litter = vec![public_key.as_str()];
        if let Some(fresh) = &new_private_key {
            litter.push(fresh.as_str());
        }
        remove_remote(resolved, &litter, runner).await;
        return Err(refused(host_channel::last_error_line(
            &registered,
            "remote registration failed",
        )));
    }

    if let Some(fresh) = &new_private_key {
        let moved = host_channel::run_program(
            resolved,
            &["/bin/mv", "-f", fresh.as_str(), private_key.as_str()],
            runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !moved.ok() {
            remove_remote(resolved, &[public_key.as_str(), fresh.as_str()], runner).await;
            return Err(refused(host_channel::last_error_line(
                &moved,
                "the new Ed25519 workload key could not be moved into place",
            )));
        }
    }
    remove_remote(resolved, &[public_key.as_str()], runner).await;

    Ok(format!(
        "{{\"status\":\"reconciled\",\"catalog\":\"{catalog_name}\"}}\n"
    ))
}

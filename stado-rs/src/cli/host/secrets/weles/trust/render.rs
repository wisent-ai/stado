use crate::cli::CmdError;

use crate::cli::host::files::forwarding::{deliver_file, DELIVERED_FILES_DIR};
use crate::cli::host::secrets::weles::trust::judge::judge_spis_trust;
use crate::cli::host::secrets::weles::trust::live_skarbiec_environment;
use crate::cli::host::secrets::weles::{catalog_file_name, remove_remote};

/// `stado host render-spis-admission-trust TARGET SOURCE` — deliver the
/// checked-in Weles renderer to TARGET and print the public Spis receipt-trust
/// document it builds there.
///
/// The point of doing it this way is what does NOT travel. The admission
/// authority's private half stays in the vault it was minted into; the
/// renderer reads the vault on the host that holds it, assembles the
/// five-field public document, and only that document crosses the channel.
/// An operator station that renders locally would have to pull the item's
/// fields to itself first, and the four it needs are public only because the
/// fifth — which the same read would expose — is not.
///
/// Two audited halves and no third way in, the shape
/// [`sync_acquisition_scopes`] established: the renderer travels through the
/// [`stream_file`] delivery channel into `$HOME/.stado/files`, owner-only and
/// checksummed on arrival, and what runs is this command's own fixed argv.
/// Unlike that command this one reaps what it delivered — the retired helper
/// channel had a writer and no reaper, and `host provenance` still counts the
/// scripts it left behind.
pub async fn render_spis_admission_trust(target: &str, source: &str) -> Result<(), CmdError> {
    use crate::deploy::host_channel;

    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage("renderer source must be a regular file"));
    }
    let name = catalog_file_name(source)?;
    let (delivered, _bytes) = deliver_file(target, source, &name).await?;

    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: the renderer reached {delivered} and produced no document: {detail}",
            resolved.name
        ))
    };

    let home = host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // `deliver_file` reports where the file landed for an operator to read —
    // with `$HOME` unexpanded, because that is the spelling the delivery
    // channel used. It is a message, not a path: quoting it for a remote test
    // asks about a directory literally named `$HOME`. The usable path is the
    // same one the channel built, composed here against the resolved home, the
    // way `register_acquisition_scopes` composes the catalog it reads.
    let installed = format!("{home}/{DELIVERED_FILES_DIR}/{name}");
    let declared = match live_skarbiec_environment(&resolved, &home, &runner).await {
        Ok(environment) => environment,
        Err(detail) => {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(detail));
        }
    };
    let vault = declared
        .iter()
        .find(|(key, _)| key == "SKARBIEC_VAULT_FILE")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");

    // The renderer is a Node program, at the interpreter this fleet's macOS
    // hosts install; a host that resolves `node` elsewhere answers for itself
    // rather than being assumed.
    let brewed = "/opt/homebrew/bin/node";
    let node = if host_channel::remote_test(&resolved, &format!("-x {brewed}"), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        brewed.to_string()
    } else {
        let looked_up = host_channel::run_command(&resolved, "command -v node", &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let found = looked_up.stdout.trim().to_string();
        if found.is_empty() {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(
                "no Node runtime is installed on this host".to_string(),
            ));
        }
        found
    };

    for file in [&skarbiec, &vault, &installed] {
        let present = host_channel::remote_test(
            &resolved,
            &format!("-f {}", crate::deploy::shlex_quote(file)),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(format!("required file is missing: {file}")));
        }
    }

    // The item id and the field names are the renderer's own compile-time
    // constants, so nothing that could name a secret field reaches this
    // command line, and no field VALUE ever does.
    //
    // The PATH is explicit for the same reason `register_acquisition_scopes`
    // sets one: Skarbiec decrypts by spawning GnuPG, and a login shell reached
    // through the channel does not necessarily carry the Homebrew prefix the
    // fleet installs it under.
    let mut assignments = vec![format!(
        "PATH={}",
        crate::deploy::shlex_quote(
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        )
    )];
    for (key, value) in &declared {
        assignments.push(format!("{key}={}", crate::deploy::shlex_quote(value)));
    }
    assignments.push(format!(
        "SKARBIEC_BIN={}",
        crate::deploy::shlex_quote(&skarbiec)
    ));
    let rendered = host_channel::run_command(
        &resolved,
        &format!(
            "{} {} {}",
            assignments.join(" "),
            crate::deploy::shlex_quote(&node),
            crate::deploy::shlex_quote(&installed),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    remove_remote(&resolved, &[installed.as_str()], &runner).await;
    if !rendered.ok() {
        return Err(refused(host_channel::last_error_line(
            &rendered,
            "the renderer refused",
        )));
    }
    judge_spis_trust(&rendered.stdout).map_err(refused)?;

    // The host's own bytes, verbatim: this document is committed to a public
    // repository and compared byte-for-byte at activation, so re-serializing
    // it here would be this command quietly authoring it.
    print!("{}", rendered.stdout);
    if !rendered.stdout.ends_with('\n') {
        println!();
    }
    Ok(())
}

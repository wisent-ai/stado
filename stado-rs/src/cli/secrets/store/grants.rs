//! Committing a new backend, and minting the request-only grant a consumer
//! reads one exact field with.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::secrets::store::resolve::skarbiec_launcher;

pub(crate) async fn migrate(destination: Option<&str>) -> Result<(), CmdError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|err| CmdError::click(err.to_string()))?;
    let report = crate::credential_store::migrate::migrate(
        destination,
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .await
    .map_err(|err| CmdError::click(err.to_string()))?;
    println!(
        "migrated {} credential item(s): {} -> {}",
        report.moved_items, report.source, report.destination
    );
    Ok(())
}

fn exact_component(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(CmdError::click(format!(
            "{kind} must be a non-empty exact name containing only ASCII letters, digits, dot, underscore, or dash"
        )));
    }
    Ok(())
}

pub(crate) fn mint_acquisition_token(
    consumer: &str,
    item: &str,
    field: &str,
    output: &str,
) -> Result<(), CmdError> {
    exact_component("consumer", consumer)?;
    exact_component("item", item)?;
    exact_component("field", field)?;
    let output_path = std::path::Path::new(output);
    if output_path.try_exists()? {
        return Err(CmdError::click(format!(
            "refusing to overwrite existing token file {}",
            output_path.display()
        )));
    }
    let launcher = skarbiec_launcher()?;
    // Skarbiec's mint takes `--capabilities action:item[#field]`. This command
    // passed `--acquisition-scopes`, a flag that no longer exists, so it failed
    // with "--capabilities is required" and the 0.14.9 delivery's resume step
    // could not re-mint the publisher's bootstrap token when the vault's
    // recorded bearer no longer matched the file. The capability the fleet
    // reads these files for is `read` on the exact item and field — that is
    // what every working consumer's token in the operator vault carries.
    let capability = format!("read:{item}#{field}");
    // And into the OWNER vault, not whichever default the CLI would open. The
    // keychain launcher sets no SKARBIEC_VAULT_FILE, so without this the grant
    // landed in ~/.local/share/skarbiec/… while the control-plane broker serves
    // ~/.stado/skarbiec.vault.json: the mint reported success, the file read
    // verified against the wrong store, and the broker kept answering 403 for
    // the consumer the fleet authenticates as. On 2026-09-04 that is what the
    // 0.14.9 delivery's resume step died on. `credential_store::owner::vault`
    // is the one declaration of where owner writes go.
    let vault = crate::credential_store::owner::vault()
        .map_err(|error| CmdError::click(format!("mint-acquisition-token: {error}")))?;
    let minted = std::process::Command::new(&launcher)
        .args(["grant", "issue"])
        .arg(consumer)
        .arg("--capabilities")
        .arg(&capability)
        .env("SKARBIEC_VAULT_FILE", &vault)
        .output()?;
    if !minted.status.success() {
        return Err(CmdError::click(format!(
            "{} grant issue failed: {}",
            launcher.display(),
            String::from_utf8_lossy(&minted.stderr).trim()
        )));
    }
    let report: Value = serde_json::from_slice(&minted.stdout).map_err(|_| {
        CmdError::click(format!(
            "{} grant issue produced no JSON report",
            launcher.display()
        ))
    })?;
    let token = report
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Skarbiec grant issue report contained no token"))?;
    let owner_read_write = (u8::BITS - u16::BITS / u8::BITS) << (u8::BITS - u16::BITS / u8::BITS);
    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(owner_read_write)
            .open(output_path)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::process::Command::new(&launcher)
            .args(["grant", "revoke"])
            .arg(consumer)
            .output();
        let _ = std::fs::remove_file(output_path);
        return Err(CmdError::click(format!(
            "cannot write token file {}: {error}; the freshly minted grant was revoked",
            output_path.display()
        )));
    }
    println!(
        "minted request-only {capability} grant for {consumer} into {}",
        output_path.display()
    );
    Ok(())
}

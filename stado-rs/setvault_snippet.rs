/// Write one vault item field from a file already on the host.
/// Uses the host-local Skarbiec binary with its own vault file.
pub async fn set_item_field_on_host(
    target: &ComputeTarget,
    item: &str,
    field: &str,
    value_file: &str,
    vault_file: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = format!(
        r#"set -eu
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GNUPGHOME="$HOME/.gnupg"
export SKARBIEC_VAULT_FILE="{vault_file}"
"$HOME/.stado/bin/skarbiec" set-json "{item}" --field "{field}" --from-file "{value_file}"
echo 'STADO_ITEM_SET\tok'"#,
        vault_file = vault_file,
        item = item,
        field = field,
        value_file = value_file,
    );
    let output = host_channel::run_script(target, &body, runner).await?;
    if !output.stdout.contains("STADO_ITEM_SET") && !output.status.success() {
        return Err(DeployError(format!(
            "{}: could not set {}.{}: {}",
            target.name,
            item,
            field,
            output.stderr.trim_end()
        )));
    }
    Ok(report_from(output))
}

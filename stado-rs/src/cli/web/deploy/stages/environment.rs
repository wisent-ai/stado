//! What the unit is given: the plain variables its rendered definition
//! carries, the Skarbiec values delivered into its owner-only env file, and
//! the bearer and grant that authorize it to hold them.

use crate::cli::web::deploy::{click, marker, WEB_ENV_FILE_VARIABLE};
use crate::cli::CmdError;
use crate::config::WebApiProduct;
use crate::deploy::{host_channel, Runner};
use crate::targets::ComputeTarget;

/// Ensure the unit's bearer file exists before its grant is reconciled.
///
/// [`crate::deploy::service::remint_consumer_grant_on_host`] preserves the
/// bearer already in the token file and refuses a path that holds no regular
/// file, so a unit being deployed for the first time has nothing to mint
/// against. The bearer is generated ON the host, the way
/// `cli/host.rs`'s verifier reconciliation generates one: a bearer this
/// machine invented would exist in the control plane's memory, and the whole
/// point of the token file is that the value never gets there.
const WEB_BEARER_BODY: &str = r#"
refuse() {
  printf '%s\n' "$1" >&2
  exit 1
}
case "$token_file" in
  "$HOME"/*) ;;
  *) refuse 'the bearer path must be under the target account home' ;;
esac
[ ! -L "$token_file" ] || refuse 'the bearer path is a symlink; a bearer written through one is a bearer somewhere else'
if [ -f "$token_file" ]; then
  /bin/chmod 600 "$token_file" || refuse 'cannot protect the existing bearer file'
  printf 'STADO_WEB_BEARER\tpresent\n'
  exit 0
fi
parent="$(/usr/bin/dirname "$token_file")"
/bin/mkdir -p "$parent" || refuse 'cannot create the bearer directory'
/bin/chmod 700 "$parent" || refuse 'cannot protect the bearer directory'
staged="$token_file.stado-new.$$"
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
umask 077
/usr/bin/openssl rand -hex 32 > "$staged" || refuse 'cannot generate a bearer on this host'
[ -s "$staged" ] || refuse 'the generated bearer is empty'
/bin/mv -f "$staged" "$token_file" || refuse 'cannot install the bearer file'
trap - EXIT HUP INT TERM
printf 'STADO_WEB_BEARER\tminted\n'
"#;

/// The environment the rendered unit carries.
///
/// `PORT` is what the launcher refuses to start without, `NODE_ENV` is what
/// Next.js reads to serve the production build, and `WEB_ENV_FILE` is how the
/// launcher finds the secrets this command delivers separately. The product's
/// own declared entries come last so a declaration can correct any of them:
/// an operator who declares `NODE_ENV` has said something deliberate, and
/// silently winning over them would make the declaration a lie.
pub(in crate::cli::web::deploy) fn unit_environment(
    declared: &WebApiProduct,
    env_file: &str,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("PORT".to_string(), declared.port().to_string()),
        ("NODE_ENV".to_string(), "production".to_string()),
        (WEB_ENV_FILE_VARIABLE.to_string(), env_file.to_string()),
    ];
    for (variable, value) in declared.env() {
        match env.iter_mut().find(|(name, _)| name == variable) {
            Some(existing) => existing.1 = value.clone(),
            None => env.push((variable.clone(), value.clone())),
        }
    }
    env
}

/// The credential item the database plane hands out for this consumer.
///
/// Resolved exactly the way `stado database resolve` resolves it — the
/// declaration out of `database_api.databases`, then
/// [`crate::config::DatabaseApiDatabase::allows_consumer`] — and refused with
/// that plane's own sentence, so a product that is not a declared consumer of
/// a database is told the same thing by both commands. The value is never
/// read here: only the item name crosses, and the field itself is delivered by
/// the same secret-sync path every other secret takes.
fn database_credential_item(database: &str, consumer: &str) -> Result<String, CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let declared = databases.get(database).ok_or_else(|| {
        CmdError::usage(format!(
            "unknown database {database:?}; declared: {}",
            databases.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    if !declared.allows_consumer(consumer) {
        return Err(CmdError::usage(format!(
            "consumer {consumer:?} is not authorized for database {database:?}"
        )));
    }
    Ok(declared.item().to_string())
}

/// Every `(variable, item, field)` this unit's environment needs, plain
/// secrets first and the database credential last.
///
/// Collected before anything is delivered so a malformed reference is refused
/// with nothing written: half a unit's environment is worse than none, because
/// the unit starts and fails somewhere the operator has to go looking.
pub(in crate::cli::web::deploy) fn secret_deliveries(
    declared: &WebApiProduct,
) -> Result<Vec<(String, String, String)>, CmdError> {
    let mut deliveries = Vec::with_capacity(declared.secrets().len() + 1);
    for (variable, reference) in declared.secrets() {
        let (item, field) = crate::config::parse_secret_reference(reference).ok_or_else(|| {
            CmdError::click(format!(
                "{variable} names the secret {reference:?}, which is not an 'item#field' \
                 reference"
            ))
        })?;
        deliveries.push((variable.clone(), item.to_string(), field.to_string()));
    }
    if let Some(database) = declared.database() {
        let item = database_credential_item(database.name(), declared.consumer())?;
        deliveries.push((
            database.variable().to_string(),
            item,
            database.field().to_string(),
        ));
    }
    Ok(deliveries)
}

/// The complete grant a web unit's consumer holds: read on exactly the fields
/// this product's declaration names, and nothing else.
///
/// Spelled `read:<item>#<field>`, the same capability grammar
/// `cli/host.rs`'s verifier reconciliation mints. Derived from the same list
/// the deliveries come from, so a grant can never be wider than the set of
/// values the unit is actually given.
pub(in crate::cli::web::deploy) fn grant_capabilities(
    deliveries: &[(String, String, String)],
) -> String {
    deliveries
        .iter()
        .map(|(_, item, field)| format!("read:{item}#{field}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Make sure the unit's bearer file is there, and say whether it had to be
/// minted.
pub(in crate::cli::web::deploy) async fn ensure_bearer(
    target: &ComputeTarget,
    token_file: &str,
    runner: &Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -eu\ntoken_file={}\n{WEB_BEARER_BODY}",
        crate::deploy::shlex_quote(token_file),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: could not prepare the unit's bearer file: {}",
            target.name,
            host_channel::last_error_line(&output, "the bearer file was not prepared")
        )));
    }
    Ok(marker(&output.stdout, "STADO_WEB_BEARER")
        .and_then(|fields| fields.first().copied())
        .unwrap_or("unknown")
        .to_string())
}

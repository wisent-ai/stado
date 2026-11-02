//! The fleet vault's own write fence, which a move of the profile carrying
//! it stands behind instead of stopping the source's unit.
//!
//! The registry authority authorizes every write to its own store with a
//! bearer whose version it asks the fleet vault for on each request, and it
//! fails closed (`dashboard/listener/auth/tokens.rs`). A move that stopped
//! the vault's unit on the authority's host could therefore never commit: its
//! last step, the registry write, asked the vault the move had stopped, and
//! the whole move rolled back after every state was copied. Skarbiec holds
//! every write for a named transaction while every read keeps answering
//! (`skarbiec fence arm|lift --transaction T`), so the copy the move takes
//! after arming it is the last one, and the authority and every consumer
//! still reading on the source keep their answers through the commit. The
//! source's units are retired after the commit, once the source host's own
//! resolver follows the committed registry to the destination.

use serde_json::Value;

use super::{run_host_script, MoveContext};
use crate::cli::host::{
    credential_host_at, release_managed_skarbiec_in, remote_config_output, remote_stado_verdict,
    RemoteConfigAction,
};
use crate::cli::placement::candidates::{managed_unit, profile_host, unit};
use crate::cli::CmdError;
use crate::deploy::{shlex_quote, Runner};
use crate::placement::PlacementProfile;

/// Whether `profile` carries the fleet vault, whose move holds writes with
/// the vault's own fence instead of stopping it.
pub(in crate::cli::placement) fn write_fenced(profile: &PlacementProfile) -> bool {
    let vault = crate::cli::secrets::VAULT_SERVICE;
    profile.services.iter().any(|service| service == vault)
}

/// The source's vault as Skarbiec is run against it there: the vault file
/// and GnuPG home the source declares, and the Skarbiec release the registry
/// manages on it.
struct SourceVault {
    skarbiec: String,
    vault: String,
    gnupg_home: String,
}

async fn source_vault(context: &MoveContext, runner: &Runner) -> Result<SourceVault, CmdError> {
    let declared = credential_host_at(context.source.clone()).await?;
    let skarbiec = release_managed_skarbiec_in(
        &context.claimed_document,
        &declared.target,
        runner,
        &declared.home,
    )
    .await?;
    Ok(SourceVault {
        skarbiec,
        vault: declared.vault,
        gnupg_home: declared.gnupg_home,
    })
}

/// `skarbiec <arguments>` on the move's source against its declared vault;
/// its stdout read as JSON.
async fn run_skarbiec(
    context: &MoveContext,
    source: &SourceVault,
    arguments: &[&str],
    runner: &Runner,
) -> Result<Value, CmdError> {
    let words: Vec<String> = arguments.iter().map(|word| shlex_quote(word)).collect();
    let script = format!(
        "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} {}\n",
        shlex_quote(&source.gnupg_home),
        shlex_quote(&source.vault),
        shlex_quote(&source.skarbiec),
        words.join(" ")
    );
    let shown = format!("skarbiec {} ({})", arguments.join(" "), source.skarbiec);
    let output = run_host_script(&context.source, &script, runner, &shown).await?;
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: {shown} printed no readable JSON: {error}: {}",
            context.source.name,
            output.stdout.trim()
        ))
    })
}

async fn source_skarbiec(
    context: &MoveContext,
    arguments: &[&str],
    runner: &Runner,
) -> Result<Value, CmdError> {
    let source = source_vault(context, runner).await?;
    run_skarbiec(context, &source, arguments, runner).await
}

/// Hold every write to the source's vault for this move's transaction. A
/// fence the same transaction already holds is the same fence.
pub(super) async fn arm(context: &MoveContext, runner: &Runner) -> Result<(), CmdError> {
    let reason = format!(
        "placement move {} of {} to {}",
        context.transaction.id, context.profile.name, context.destination.name
    );
    let answer = source_skarbiec(
        context,
        &[
            "fence",
            "arm",
            "--transaction",
            &context.transaction.id,
            "--reason",
            &reason,
        ],
        runner,
    )
    .await?;
    if answer.get("armed").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::click(format!(
            "{}: skarbiec fence arm answered {answer}, not an armed fence",
            context.source.name
        )));
    }
    Ok(())
}

/// Let writes reach the source's vault again: what a move that did not
/// commit leaves behind. A vault no fence stands on is already open.
pub(super) async fn lift(context: &MoveContext, runner: &Runner) -> Result<(), CmdError> {
    source_skarbiec(
        context,
        &["fence", "lift", "--transaction", &context.transaction.id],
        runner,
    )
    .await
    .map(drop)
}

/// The refusals a vault move must give before anything changes, since each
/// would otherwise stop it only after every state was copied, or let writes
/// through the fence: a source Skarbiec that has no write fence; a vault
/// unit that does not run that Skarbiec, or whose live process is not
/// executing the file it declares (an old process ignores the fence on its
/// writes); and a source whose own Stado process reads the vault at the unit
/// the move retires, which would leave the registry authority and the object
/// API on that host without a vault.
pub(super) async fn ensure_fenceable(
    context: &MoveContext,
    runner: &Runner,
) -> Result<(), CmdError> {
    let source = &context.source.name;
    let vault_source = source_vault(context, runner).await?;
    run_skarbiec(context, &vault_source, &["fence", "show"], runner)
        .await
        .map_err(|error| {
            CmdError::refused(format!(
                "{source}: its Skarbiec cannot hold writes for a move ({error}); a move of the \
                 fleet vault keeps it answering reads behind `skarbiec fence arm`, so roll out a \
                 Skarbiec release that has `skarbiec fence` on {source} first"
            ))
        })?;
    let source_profile = profile_host(&context.profile, source)?;
    let vault = crate::cli::secrets::VAULT_SERVICE;
    let declared_unit = managed_unit(unit(source_profile, vault)?)?;
    let unit_file = run_host_script(
        &context.source,
        &format!("cat -- {}\n", shlex_quote(&declared_unit.path)),
        runner,
        &format!("reading {}", declared_unit.path),
    )
    .await?;
    if !unit_file.stdout.contains(&vault_source.skarbiec) {
        return Err(CmdError::refused(format!(
            "{source}: unit {} ({}) does not run {}, the Skarbiec whose write fence the move \
             arms, so whether the serving process holds writes behind it cannot be told; put \
             the unit on the Skarbiec release the registry manages there first",
            declared_unit.unit, declared_unit.path, vault_source.skarbiec
        )));
    }
    serves_installed_image(context, &declared_unit.unit, runner).await?;
    let Some(probe) = source_profile.probes.iter().find(|probe| probe.service == vault) else {
        return Err(CmdError::declaration(format!(
            "placement profile {} declares no probe of {vault} on {source}",
            context.profile.name
        )));
    };
    let configuration = remote_config_output(&context.source, RemoteConfigAction::Show, runner)
        .await?;
    let document: Value = serde_json::from_str(&configuration).map_err(|error| {
        CmdError::click(format!(
            "{source}: its configuration answer is not JSON, so where its Stado reads the vault \
             cannot be told: {error}"
        ))
    })?;
    let reads = document["resolved"]["skarbiec_url"].as_str().unwrap_or_default();
    if same_endpoint(reads, &probe.url) {
        return Err(CmdError::refused(format!(
            "{source}: its Stado reads the fleet vault at {reads}, the {vault} unit this move \
             retires, so the registry authority and the object API there would lose the vault \
             after the commit; point it at its resolver adapter with `stado host config set \
             {source} secrets.skarbiec.url stado://service/{vault}?consumer=stado \
             --reload-service stado` run on {source}, whose owner vault holds the host account \
             the reload of a system daemon needs, or move the registry authority and the \
             stado-object-api role off {source} first"
        )));
    }
    Ok(())
}

/// The source's own `registry doctor` (the only machine that can read which
/// image a process executes) must not report the vault unit's live process
/// as running another file than its unit declares, nor as unread: a process
/// started before its Skarbiec was replaced ignores the fence on its writes.
async fn serves_installed_image(
    context: &MoveContext,
    label: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let source = &context.source.name;
    let output =
        remote_stado_verdict(&context.source, &["registry", "doctor", "--json"], runner).await?;
    let report: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{source}: stado registry doctor --json answered no readable report (exit {}): \
             {error}; {}",
            output.code,
            output.detail().trim()
        ))
    })?;
    let whole_host = format!("the executing image of every unit on {source}");
    let about_unit = format!("{label} ");
    let findings = report["findings"].as_array().map(Vec::as_slice).unwrap_or_default();
    for finding in findings {
        let kind = finding["finding"].as_str().unwrap_or_default();
        let detail = finding["detail"].as_str().unwrap_or_default();
        if (kind == "stale-unit-image" || kind == "unread-unit-image")
            && finding["subject"].as_str() == Some(source.as_str())
            && (detail.starts_with(&about_unit) || detail.starts_with(&whole_host))
        {
            return Err(CmdError::refused(format!(
                "{source}: its own registry doctor reports {kind} for the vault unit: {detail}; \
                 a process not executing its installed Skarbiec ignores the write fence, so put \
                 {label} on its installed file (`stado service image refresh {label}` on \
                 {source}) before moving"
            )));
        }
    }
    Ok(())
}

/// Whether two URLs name the same host and port.
fn same_endpoint(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (url::Url::parse(left), url::Url::parse(right)) else {
        return false;
    };
    left.host_str() == right.host_str() && left.port_or_known_default() == right.port_or_known_default()
}

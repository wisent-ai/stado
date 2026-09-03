//! `stado web deploy` — a declared web product's published release, running
//! as a managed unit with its environment delivered from Skarbiec.
//!
//! Nothing here is a new way to run a service. The unit is rendered by
//! [`crate::deploy::service::plan_deploy_labelled`], installed by
//! [`crate::deploy::service::ensure_service`], restarted by
//! [`crate::deploy::service::restart_service`] and recorded through
//! `cli/registry.rs`'s validated conditional write — the same four steps
//! `stado service ensure` takes. Every secret arrives through
//! [`crate::deploy::service::sync_service_secret`], the one path a Skarbiec
//! value is allowed to reach a host by, and the consumer grant is reconciled
//! by [`crate::deploy::service::remint_consumer_grant_on_host`]. A database
//! credential is resolved for the unit's own consumer against
//! `database_api.databases`, so a product the database does not list is
//! refused with the database plane's own sentence rather than handed a
//! credential it was never granted.
//!
//! What IS owned here is the one thing no other module could do: turning a
//! published web release into an install root. A web recipe stages one
//! tarball (`<product>-web.tar.gz`) plus its digest sidecar, and the release
//! pipeline packages the stage map into `release.tar.gz`, so the bytes a web
//! unit runs sit one archive inside another. [`WEB_INSTALL_BODY`] is that
//! double unpack, and it exists rather than reusing one of the two installers
//! beside it for reasons that are properties of those installers:
//! [`crate::deploy::artifact_install::install_artifact`] resolves an
//! `stado artifact` manifest, which a pipeline release does not publish, and
//! `cli/service.rs`'s archive install is private, stages through `scp` from
//! this machine, and hardcodes `darwin-arm` as the platform directory. The
//! program the unit runs is composed with `$HOME` and `$STADO_PLATFORM` and
//! expanded by [`crate::deploy::service_catalog::resolve_word`], which is the
//! same expansion `data/service-catalog.json` already uses for brama, so the
//! layout a web release lands in is the layout the fleet already has.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::CmdError;
use crate::config::WebApiProduct;
use crate::declaration::{DeclarationRun, DeclarationSource, ServiceDeclaration};
use crate::deploy::service::{self, ManagedService};
use crate::deploy::{host_channel, production_runner, service_catalog, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The one platform key a web product's `.wisent-release.json` declares.
///
/// A web product is built once, on whichever host its recipe names as
/// `runner_platform`, and the bytes are a Node tree that runs on either
/// platform — so the coordinate the release is published under is `web` and
/// not the builder's triple. Naming it here once is what stops the version
/// lookup and the artifact lookup from disagreeing about where a web release
/// lives.
const WEB_PLATFORM: &str = "web";

/// Where a web unit's owner-only runtime environment file lives, under the
/// target account's home.
///
/// The unit file carries `PORT`, `NODE_ENV` and the product's declared plain
/// environment, because those are declarations an operator wrote and the
/// registry already holds. Every secret goes here instead: a value in a
/// launchd plist is a value in the canonical registry document, readable by
/// anything that can read the registry, and `stado service secret-sync`
/// exists precisely so a credential lands in a mode-600 file on one host and
/// nowhere else. The launcher sources this file, and the unit tells it where
/// to look through `WEB_ENV_FILE`, so the path is this module's to choose and
/// the launcher stays free of a baked-in location.
const WEB_ENV_DIR: &str = ".stado/web";

/// The variable the unit passes the launcher so it knows which env file to
/// source. Without it every delivered secret is dead text: the launcher would
/// `exec npm run start` with none of them in its environment, and this
/// command would report them delivered.
const WEB_ENV_FILE_VARIABLE: &str = "WEB_ENV_FILE";

/// Where a web unit's Skarbiec bearer lives. The grant reconciler records only
/// this file's hash, and the file itself never leaves the host.
const WEB_TOKEN_DIR: &str = ".stado/web/tokens";

/// The authoritative Skarbiec vault on a managed host — the same default
/// `stado service grant-sync` carries, spelled once so a web unit's grant and
/// every other unit's grant are minted against one vault.
const VAULT_FILE: &str = "$HOME/.stado/skarbiec.vault.json";

/// Lifetime of a web unit's consumer grant, matching `service grant-sync`'s
/// own default of thirty days. A shorter grant would expire between releases
/// of a product that ships monthly; a longer one outlives the operator's
/// memory of having minted it.
const GRANT_TTL_SECONDS: u64 = 2_592_000;

/// How many times the readiness probe asks before it gives up, and how long it
/// waits between asks.
///
/// Bounded and stated rather than "until it works": a Next.js server that
/// cannot start does not start on the tenth try either, and an unbounded wait
/// turns a failed deploy into a command that never returns. Twenty attempts
/// three seconds apart is a minute of grace, which is longer than every cold
/// start measured on the fleet's mac mini and short enough that a broken unit
/// is reported while the operator is still watching.
const READY_ATTEMPTS: u32 = 20;
const READY_INTERVAL_SECONDS: u32 = 3;
/// Per-request budget for one readiness attempt.
const READY_REQUEST_SECONDS: u32 = 5;

/// A release archive carries a production `node_modules`, so it is tens of
/// megabytes and the fetch happens on the host. The short host-channel clock
/// is right for a probe and wrong for this, exactly as
/// [`crate::deploy::host_release`] found for its own staging phase.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn click(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

/// The release run states that mean the bytes are published and promoted.
///
/// `promoted` is the state `stado release submit` leaves a run in once the
/// channel pointer has moved; `reconciled` and `completed` are the two
/// terminal states past it. Anything earlier — `submitting`, `waiting`,
/// `publishing`, `delivering` — is a build in flight, and deploying from one
/// would install bytes whose qualification has not been decided.
const PUBLISHED_RUN_STATES: [&str; 3] = ["promoted", "reconciled", "completed"];

/// The newest published stable version of one product, from the release run
/// objects `stado release submit` maintains.
///
/// [`crate::cli::release_submit::recent_runs`] is the read side of those
/// objects — the same reader `stado release status` and the operator console
/// print — and it answers newest-first, so the first row that is both
/// `stable` and published is the answer. Nothing here parses a version
/// string to compare it: the run objects are ordered by the store's own write
/// time, and a product whose version numbers went backwards still deployed
/// whatever was promoted last, which is what "current" means.
///
/// The refusal names the product and what was found, because the two ways
/// this fails have different repairs: a product with no run at all has never
/// been submitted, and a product whose newest stable run is still publishing
/// has to finish.
async fn published_stable_version(product: &str) -> Result<String, CmdError> {
    /// How far back the search looks. A product's own runs are already
    /// filtered by `recent_runs`, so this is a bound on how many of ITS runs
    /// are examined, not on the fleet's history.
    const RUN_WINDOW: usize = 32;

    let runs = crate::cli::release_submit::recent_runs(Some(product), RUN_WINDOW).await?;
    if runs.is_empty() {
        return Err(CmdError::click(format!(
            "no release run has ever been submitted for {product}; run \
             `stado release submit {product} --channel stable` first, or name an exact \
             `--version`"
        )));
    }
    let published = runs.iter().find(|run| {
        run["channel"].as_str() == Some("stable")
            && run["state"]
                .as_str()
                .is_some_and(|state| PUBLISHED_RUN_STATES.contains(&state))
    });
    let Some(run) = published else {
        let newest = runs.first().expect("a non-empty run list has a first row");
        return Err(CmdError::click(format!(
            "{product} has no published stable release; its newest run is {} on the {} channel \
             in state {}. Promote a stable release, or name an exact `--version`",
            newest["version"].as_str().unwrap_or("an unnamed version"),
            newest["channel"].as_str().unwrap_or("unknown"),
            newest["state"].as_str().unwrap_or("unknown"),
        )));
    };
    run["version"]
        .as_str()
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the newest published stable release run for {product} carries no version"
            ))
        })
}

/// Place one published web release into the product's install root, verify
/// both archives against the digests that declare them, and point `current`
/// at the result.
///
/// Order is the whole design, and it is the order
/// [`crate::deploy::host_release`] states: nothing touches `current` until
/// both digests have matched and the launcher has been found. A failed fetch,
/// a short body, a tampered inner tarball or a tarball with no launcher in it
/// each leave the running release exactly where it was, because the running
/// release has not been opened.
///
/// The inner sidecar is not belt-and-braces. The release archive's digest is
/// the release plane's statement about the release archive; the sidecar is the
/// build's statement about the tarball the unit actually runs, and they are
/// produced by different steps on different machines. Checking only the outer
/// one would accept a release archive that was assembled correctly around a
/// web tarball the build wrote badly.
const WEB_INSTALL_BODY: &str = r#"
refuse() {
  printf '%s\n' "$1" >&2
  exit 1
}

digest() {
  if command -v /usr/bin/shasum >/dev/null 2>&1; then
    /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | /usr/bin/awk '{print $1}'
  else
    refuse 'this host has neither shasum nor sha256sum, so nothing here can be verified'
  fi
}

root="$HOME/.stado/services/$name"
version_dir="$root/$version"
release_dir="$version_dir/.release"
download="$version_dir/.release.tar.gz"

/bin/mkdir -p "$version_dir" || refuse "cannot create $version_dir"
/bin/rm -rf "$release_dir"
/bin/mkdir -p "$release_dir" || refuse "cannot create $release_dir"

if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi

case "$archive_uri" in
  stado://*)
    [ -n "$stado_bin" ] || refuse "$archive_uri needs stado on this host to read the release channel"
    "$stado_bin" storage cat "$archive_uri" > "$download" \
      || refuse "the release channel did not serve $archive_uri"
    ;;
  https://*)
    /usr/bin/curl -fsSL --retry 3 "$archive_uri" -o "$download" \
      || refuse "$archive_uri could not be fetched"
    ;;
  *)
    refuse "release location $archive_uri is neither the fleet release channel nor https"
    ;;
esac

observed="$(digest "$download")"
if [ "$observed" != "$expected" ]; then
  /bin/rm -f "$download"
  refuse "release archive digest mismatch: the manifest declares $expected, the host received $observed"
fi

/usr/bin/tar -xzf "$download" -C "$release_dir" || refuse 'the release archive did not unpack'
/bin/rm -f "$download"

set -- "$release_dir"/*-web.tar.gz
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  refuse "the release archive stages $# web tarballs; a web release stages exactly one *-web.tar.gz"
fi
inner="$1"
sidecar="$inner.sha256"
[ -f "$sidecar" ] || refuse "the release archive carries no digest sidecar beside $(/usr/bin/basename "$inner")"
declared="$(/usr/bin/awk '{print $1; exit}' "$sidecar")"
[ -n "$declared" ] || refuse 'the digest sidecar is empty'
observed="$(digest "$inner")"
if [ "$declared" != "$observed" ]; then
  refuse "web tarball digest mismatch: the build declares $declared, the archive holds $observed"
fi

stage="$version_dir/$platform"
/bin/rm -rf "$stage"
/bin/mkdir -p "$stage" || refuse "cannot create $stage"
# The tarball carries exactly one top-level `<product>-<version>/` directory,
# which is what stops an extraction from scattering node_modules across
# whatever directory tar happened to run in. Stripping it here is what makes
# the launcher land where the unit's program path says it is.
/usr/bin/tar -xzf "$inner" --strip-components=1 -C "$stage" \
  || refuse 'the web tarball did not unpack'
/bin/rm -rf "$release_dir"

[ -f "$stage/$launcher" ] || refuse "the web tarball carries no $launcher, so the unit would have nothing to run"
/bin/chmod u+x "$stage/$launcher" || refuse "cannot make $launcher executable"

# `current` is a symlink on every host this installs to, but a directory on
# hosts an older installer touched; either way the previous release is kept
# beside the new one rather than deleted, so a rollback is a relink.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version" || refuse 'cannot retire the previous release directory'
else
  /bin/rm -f "$root/current"
fi
/bin/ln -sfn "$version_dir" "$root/.current.new" || refuse 'cannot stage the current link'
/bin/mv -f "$root/.current.new" "$root/current" || refuse 'cannot publish the current link'
printf 'STADO_WEB_INSTALL\t%s\n' "$version_dir"
"#;

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

/// Poll the unit's readiness path from the host itself.
///
/// It has to run there. The declared port is loopback — the launcher binds
/// `127.0.0.1` deliberately, because the public entrance is the edge and not
/// the unit — so a probe from the operator's laptop reaches nothing, and a
/// probe from the tailnet address would be answering a different question.
///
/// The loop is on the host rather than in this process for the same reason
/// the fetch is: one round trip instead of twenty, and the whole wait is
/// bounded by the script's own attempt count rather than by how long an ssh
/// connection happens to survive.
///
/// The last state is carried out, not just the verdict. "Nothing answered on
/// the port" and "answered HTTP 503" are opposite findings with opposite
/// repairs: the first says the process is not listening, the second says it is
/// listening and telling you it is not ready.
const WEB_READY_BODY: &str = r#"
attempt=0
last='no attempt was made'
while [ "$attempt" -lt "$attempts" ]; do
  attempt=$((attempt + 1))
  code="$(/usr/bin/curl -s -o /dev/null -m "$request_timeout" -w '%{http_code}' "$url" 2>/dev/null || true)"
  if [ "$code" = "200" ]; then
    printf 'STADO_WEB_READY\tready\tHTTP 200 after %s attempt(s)\n' "$attempt"
    exit 0
  fi
  if [ -z "$code" ] || [ "$code" = "000" ]; then
    last='nothing answered on the port'
  else
    last="answered HTTP $code"
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    /bin/sleep "$interval"
  fi
done
printf 'STADO_WEB_READY\tunready\t%s after %s attempt(s)\n' "$last" "$attempt"
"#;

/// One tab-delimited marker's fields, in the protocol every remote program in
/// `deploy/` reports through.
fn marker<'a>(stdout: &'a str, key: &str) -> Option<Vec<&'a str>> {
    stdout.lines().find_map(|line| {
        let fields = host_channel::marker_fields(line);
        (fields.first() == Some(&key)).then(|| fields[1..].to_vec())
    })
}

/// The platform directory a release lands in on this host, by the same
/// shortening [`crate::deploy::service_catalog::resolve_word`] applies to
/// `$STADO_PLATFORM`. Derived through that function rather than restated, so
/// the directory the installer creates and the directory the unit's program
/// path names cannot drift apart.
fn platform_directory(target: &ComputeTarget) -> String {
    service_catalog::resolve_word(
        "$STADO_PLATFORM",
        "",
        Some(&target.release_platform),
        &target.name,
    )
}

/// The absolute program a web unit runs, with `$HOME` and `$STADO_PLATFORM`
/// resolved against the host that will run it.
fn launcher_program(target: &ComputeTarget, home: &str, product: &str) -> String {
    service_catalog::resolve_word(
        &format!(
            "$HOME/.stado/services/{product}/current/$STADO_PLATFORM/{}",
            super::LAUNCHER
        ),
        home,
        Some(&target.release_platform),
        &target.name,
    )
}

/// The environment the rendered unit carries.
///
/// `PORT` is what the launcher refuses to start without, `NODE_ENV` is what
/// Next.js reads to serve the production build, and `WEB_ENV_FILE` is how the
/// launcher finds the secrets this command delivers separately. The product's
/// own declared entries come last so a declaration can correct any of them:
/// an operator who declares `NODE_ENV` has said something deliberate, and
/// silently winning over them would make the declaration a lie.
fn unit_environment(declared: &WebApiProduct, env_file: &str) -> Vec<(String, String)> {
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
fn secret_deliveries(declared: &WebApiProduct) -> Result<Vec<(String, String, String)>, CmdError> {
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
fn grant_capabilities(deliveries: &[(String, String, String)]) -> String {
    deliveries
        .iter()
        .map(|(_, item, field)| format!("read:{item}#{field}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Install the release into the product's own service directory on the host.
async fn install_release(
    target: &ComputeTarget,
    product: &str,
    version: &str,
    archive_uri: &str,
    sha256: &str,
    runner: &Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -eu\nname={}\nversion={}\nplatform={}\narchive_uri={}\nexpected={}\nlauncher={}\n{WEB_INSTALL_BODY}",
        crate::deploy::shlex_quote(product),
        crate::deploy::shlex_quote(version),
        crate::deploy::shlex_quote(&platform_directory(target)),
        crate::deploy::shlex_quote(archive_uri),
        crate::deploy::shlex_quote(sha256),
        crate::deploy::shlex_quote(super::LAUNCHER),
    );
    let output = host_channel::run_script_with_timeout(target, &script, INSTALL_TIMEOUT, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: could not install {product} {version}: {}",
            target.name,
            host_channel::last_error_line(&output, "the release did not install")
        )));
    }
    marker(&output.stdout, "STADO_WEB_INSTALL")
        .and_then(|fields| fields.first().copied())
        .filter(|directory| !directory.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: the installer reported no version directory for {product} {version}, so \
                 what `current` points at was never observed",
                target.name
            ))
        })
}

/// Make sure the unit's bearer file is there, and say whether it had to be
/// minted.
async fn ensure_bearer(
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

/// Ask the host whether the unit answers on its own readiness path, bounded.
///
/// Returns `(verdict, detail)` rather than a bare boolean because the detail
/// is the whole value of the check: the caller turns an unready unit into a
/// refusal that names the port, the path and the last thing the host saw.
async fn wait_until_ready(
    target: &ComputeTarget,
    url: &str,
    runner: &Runner,
) -> Result<(String, String), CmdError> {
    let script = format!(
        "set -eu\nurl={}\nattempts={READY_ATTEMPTS}\ninterval={READY_INTERVAL_SECONDS}\nrequest_timeout={READY_REQUEST_SECONDS}\n{WEB_READY_BODY}",
        crate::deploy::shlex_quote(url),
    );
    // The host's own worst case, plus the channel's setup: every attempt may
    // spend its full request budget and then sleep. A shorter budget here
    // would kill the probe mid-wait and report a timeout as an unready unit.
    let budget = Duration::from_secs(u64::from(
        READY_ATTEMPTS * (READY_REQUEST_SECONDS + READY_INTERVAL_SECONDS) + 30,
    ));
    let output = host_channel::run_script_with_timeout(target, &script, budget, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the readiness probe could not be run: {}",
            target.name,
            host_channel::last_error_line(&output, "the probe reported nothing")
        )));
    }
    let fields = marker(&output.stdout, "STADO_WEB_READY").ok_or_else(|| {
        CmdError::click(format!(
            "{}: the readiness probe returned no verdict, so whether {url} answers was never \
             established",
            target.name
        ))
    })?;
    let verdict = fields.first().copied().unwrap_or("unknown").to_string();
    let detail = fields
        .get(1)
        .copied()
        .filter(|detail| !detail.is_empty())
        .unwrap_or("the probe reported no detail")
        .to_string();
    Ok((verdict, detail))
}

/// The directory entry and the managed record this deploy leaves behind, in
/// one validated conditional write.
///
/// Both halves in one update, for the reason `service declare` states: the
/// directory contract binds a fixed route to the managed unit on its active
/// host, and a directory entry pointing at no managed service is refused by
/// the validator. The publication counter advances with the entry, because a
/// consumer holding a cached copy otherwise reads a generation telling it the
/// copy is current.
///
/// The transform is pure — the declaration and the record are already decided
/// by the time it runs — which is exactly the shape
/// [`crate::cli::registry::commit_document`] is for: a lost race re-applies
/// the same entry to the newer document.
async fn record_declaration(
    product: &str,
    declared: &WebApiProduct,
    record: &ManagedService,
    declaration: &ServiceDeclaration,
) -> Result<String, CmdError> {
    let problems = crate::declaration::validate(
        &format!("service_directory.services.{product}"),
        declaration,
    );
    if !problems.is_empty() {
        return Err(CmdError::click(problems.join("; ")));
    }
    let declaration_value = serde_json::to_value(declaration)?;
    let host = declared.host().to_string();
    let port = declared.port();
    let consumer = declared.consumer().to_string();
    let readyz = declared.readyz().to_string();
    let product = product.to_string();
    let record = record.clone();
    crate::cli::registry::commit_document(move |current| {
        let mut document = current.clone();
        let directory = document
            .get_mut("service_directory")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(
                    "registry has no service_directory; an authority must publish it before a \
                     web product can be declared",
                )
            })?;
        let services = directory
            .entry("services")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("service_directory.services: must be an object"))?;
        let entry = services
            .entry(product.clone())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "service_directory.services.{product}: must be an object"
                ))
            })?;
        entry.insert("active_host".to_string(), json!(&host));
        // The endpoint map is keyed by the active host's registry name, which
        // is a value rather than a literal, so it is built as a map instead of
        // through a JSON literal.
        let mut endpoints = Map::new();
        endpoints.insert(
            host.clone(),
            json!({"url": format!("http://127.0.0.1:{port}")}),
        );
        entry.insert("endpoints".to_string(), Value::Object(endpoints));
        entry.insert("managed_service".to_string(), json!(&product));
        // The unit's own consumer is the caller the directory publishes: a web
        // product reads its own environment and nothing else reads it through
        // this entry. An entry with no consumers answers "who may call this"
        // with silence, which the directory contract refuses.
        let mut consumers = Map::new();
        consumers.insert(consumer.clone(), json!({"capabilities": []}));
        entry.insert("consumers".to_string(), Value::Object(consumers));
        entry.insert(
            "verify".to_string(),
            json!({"kind": "http", "path": &readyz, "expect_status": 200}),
        );
        entry.insert("declaration".to_string(), declaration_value.clone());

        // Converge rather than insist: a redeploy replaces the record a
        // previous pass wrote, and a first deploy adds it. `add_service`
        // refuses a name it already manages and `replace_service` refuses one
        // it does not, so trying the replacement first is what makes this
        // command re-runnable.
        if service::replace_service(&mut document, &record).is_err() {
            service::add_service(&mut document, &record).map_err(click)?;
        }
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await
}

pub(crate) async fn deploy(name: &str, version: Option<&str>, json: bool) -> Result<(), CmdError> {
    let declared = super::product(name)?;
    let host = declared.host();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();

    // The exact coordinate first, before anything is touched. An operator who
    // named `--version` gets that version and no resolution at all; without it
    // the release plane answers, and a product with nothing published is
    // refused here rather than after a host has been changed.
    let version = match version {
        Some(version) => version.to_string(),
        None => published_stable_version(name).await?,
    };
    // The signed release manifest is the only source of the digest a host must
    // reproduce, and this is the function every other consumer of a pipeline
    // release reads it through: it validates the manifest's immutable
    // identity, checks qualification, verifies the signature against a key
    // the registry trusts, and confirms the archive matches. A release that
    // fails any of those never reaches a host.
    let artifact =
        crate::cli::release_cmd::verified_artifact_for_submit(name, &version, WEB_PLATFORM).await?;

    // Every operator-facing path resolves against the host's own account, not
    // this machine's: a target's home is its own business.
    let home = host_channel::remote_home(&target, &runner)
        .await
        .map_err(click)?;
    let env_file = format!("{home}/{WEB_ENV_DIR}/{name}.env");
    let token_file = format!("{home}/{WEB_TOKEN_DIR}/{name}.token");
    let program = launcher_program(&target, &home, name);
    let environment = unit_environment(declared, &env_file);

    // Refuse a malformed secret reference and an unauthorized database before
    // the release lands, so a product whose declaration cannot be satisfied
    // does not leave a half-configured unit behind.
    let deliveries = secret_deliveries(declared)?;

    let version_directory = install_release(
        &target,
        name,
        &version,
        &artifact.archive_uri,
        &artifact.artifact_sha256,
        &runner,
    )
    .await?;

    // The unit itself, rendered and installed by the same engine
    // `stado service ensure` uses. `ensure` rather than `deploy`, because
    // `stado web deploy` is how every subsequent release lands too: it
    // installs the unit where the host does not have it, converges a drifted
    // unit file in place, and never unloads a healthy job.
    let label = super::unit_label(name);
    let plan = service::plan_deploy_labelled(&target, name, &label, &program, &[], &environment)
        .map_err(click)?;
    let outcome = service::ensure_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !outcome.succeeded() {
        return Err(CmdError::click(format!(
            "{host}: could not install the unit {label}: {}",
            outcome.report.failure()
        )));
    }
    let record = service::record_from_ensure(host, name, &outcome, &now());

    // The grant before the secrets: the unit's Skarbiec identity is what
    // authorizes it to hold them, and minting it afterwards would leave a
    // window in which the values are on the host and nothing says who may
    // read them.
    let bearer = ensure_bearer(&target, &token_file, &runner).await?;
    let capabilities = grant_capabilities(&deliveries);
    let grant = service::remint_consumer_grant_on_host(
        &target,
        declared.consumer(),
        &capabilities,
        &token_file,
        VAULT_FILE,
        GRANT_TTL_SECONDS,
        declared.consumer(),
        &runner,
    )
    .await
    .map_err(click)?;
    if !grant.succeeded("grant_synced") {
        return Err(CmdError::click(format!(
            "{host}: could not mint the Skarbiec grant for consumer {}: {}",
            declared.consumer(),
            grant.failure()
        )));
    }

    // One field of one item into one variable, over the host channel, for
    // every entry. The value is read through the isolated service-verifier
    // grant, travels only inside the channel's request body, and is dropped
    // the moment it has been written: nothing below ever prints, logs or
    // returns it, and only the variable NAMES reach the report.
    let mut delivered: Vec<String> = Vec::with_capacity(deliveries.len());
    for (variable, item, field) in &deliveries {
        let secret = crate::cli::service::service_secret(item, field).await?;
        let synced =
            service::sync_service_secret(&target, &record, &env_file, variable, &secret, &runner)
                .await
                .map_err(click)?;
        drop(secret);
        if !synced.succeeded("secret_synced") {
            return Err(CmdError::click(format!(
                "{host}: could not deliver {variable} into {env_file}: {}",
                synced.failure()
            )));
        }
        delivered.push(variable.clone());
    }

    // Restart unconditionally, and only now. The program path is identical
    // across releases — it goes through `current` on purpose, so a new
    // release moves every unit forward without re-rendering any of them — so
    // `ensure` correctly reports a unit already running the declared program
    // as already correct and touches nothing. That is exactly the case where
    // "already running the declared program" is not "already correct": the
    // program is the same file and both the bytes behind `current` and the
    // env file it sources have changed underneath it.
    let restarted = service::restart_service(&target, &record, &runner)
        .await
        .map_err(click)?;
    if !restarted.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{host}: {label} did not restart: {}",
            restarted.failure()
        )));
    }

    let readyz_url = format!("http://127.0.0.1:{}{}", declared.port(), declared.readyz());
    let (readiness, readiness_detail) = wait_until_ready(&target, &readyz_url, &runner).await?;
    if readiness != "ready" {
        return Err(CmdError::click(format!(
            "{host}: {label} never answered 200 on port {} at {} — {readiness_detail}. The unit \
             is installed and its environment is delivered; `stado service logs {name} --host \
             {host}` is where the reason is.",
            declared.port(),
            declared.readyz(),
        )));
    }

    let declaration = ServiceDeclaration {
        source: DeclarationSource {
            artifact: artifact.archive_uri.clone(),
            sha256: artifact.artifact_sha256.clone(),
            extra: Map::new(),
        },
        run: DeclarationRun {
            program: Some(program.clone()),
            args: Vec::new(),
            env: environment.iter().cloned().collect(),
            extra: Map::new(),
        },
        extra: Map::new(),
    };
    let generation = record_declaration(name, declared, &record, &declaration).await?;

    // The database credential is appended last by `secret_deliveries`, so it
    // is that list's final entry when the product declares one. Read from
    // there rather than resolved a second time: two resolutions of the same
    // consumer against the same declaration are two answers that can differ.
    let database_item = declared
        .database()
        .and_then(|_| deliveries.last().map(|(_, item, _)| item.clone()));
    let report = json!({
        "product": name,
        "host": host,
        "unit": record.unit_id(),
        "unit_domain": super::UNIT_DOMAIN,
        "port": declared.port(),
        "version": &version,
        "artifact": &artifact.archive_uri,
        "artifact_sha256": &artifact.artifact_sha256,
        "version_directory": &version_directory,
        "program": &program,
        "consumer": declared.consumer(),
        "bearer": &bearer,
        "capabilities": &capabilities,
        "env_file": &env_file,
        "variables": &delivered,
        "database_item": &database_item,
        "readiness": &readiness,
        "readiness_detail": &readiness_detail,
        "readiness_url": &readyz_url,
        "unit_action": &outcome.action,
        "registry_generation": &generation,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name}: {} on {host} port {} running {version} (sha256 {})",
            record.unit_id(),
            declared.port(),
            artifact.artifact_sha256,
        );
        println!("  program:   {program}");
        println!("  consumer:  {} (bearer {bearer})", declared.consumer());
        println!(
            "  variables: {}",
            if delivered.is_empty() {
                "none declared".to_string()
            } else {
                delivered.join(", ")
            }
        );
        if let Some(database) = declared.database() {
            println!(
                "  database:  {} field {} as {} from item {}",
                database.name(),
                database.field(),
                database.variable(),
                database_item.as_deref().unwrap_or("-"),
            );
        }
        println!("  readiness: {readiness} — {readiness_detail} at {readyz_url}");
    }
    Ok(())
}

/// `datetime.now(timezone.utc).isoformat()`, as every other writer in the
/// crate stamps a managed-service record.
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Stop one web product's unit and take it out of management.
///
/// A unit that is not there is `unchanged` rather than an error, because this
/// is the half of `stado web remove` that has to be able to run twice: an
/// operator whose first removal failed at the DNS record must be able to run
/// the whole command again, and a declaration whose unit was never deployed
/// still has to be removable.
///
/// Both registry halves go in one update. `service declare` writes the
/// directory entry and the target's managed record together, and the
/// validator correctly refuses a directory entry pointing at no managed
/// service — so dropping only one of them makes the document unwritable by
/// anything else. The publication counter advances with the directory change
/// for the same reason it advances on the way in.
pub(crate) async fn retire(name: &str, declared: &WebApiProduct) -> Result<Value, CmdError> {
    let host = declared.host();
    let label = super::unit_label(name);
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // The target's own declaration, read locally. `declared_matching` raises
    // for an empty result, and "the unit is not there" is the answer this
    // function is required to give rather than an error to raise.
    let found = service::declared_services(&target)
        .into_iter()
        .find(|candidate| candidate.matches(name) || candidate.matches(&label));

    let Some(found) = found else {
        return Ok(json!({
            "unit": label,
            "host": host,
            "change": "unchanged",
            "detail": format!("{host} does not manage {label}"),
        }));
    };

    // A placeholder record — written by a declaration that no deploy has
    // followed — names no unit and no file, so asking launchd to boot out an
    // empty label would fail on a state that is registry-only by design.
    if !(found.unit_id().is_empty() && found.path.is_empty()) {
        let runner = production_runner();
        let sudo_password = if crate::deploy::service::UnitDomain::from_path(&found.path)
            .requires_privileged_bootstrap()
        {
            crate::cli::service::host_sudo_password(&target).await?
        } else {
            None
        };
        let report = service::retire_service(&target, &found, sudo_password.as_deref(), &runner)
            .await
            .map_err(click)?;
        if !report.succeeded("retired") {
            // Forgetting a unit that is still serving is the state this whole
            // command family exists to prevent, so the declaration stays until
            // the host confirms the unit is stopped.
            return Err(CmdError::click(format!(
                "{host}: could not stop {}: {}; it is still declared in the registry",
                found.unit_id(),
                report.failure()
            )));
        }
    }

    // Expected generation: this read, taken after the unit was stopped. The
    // stop is not repeatable, so a lost race is reported rather than retried —
    // re-applying the removal against a newer document would erase whatever
    // the winning writer said about this service while the host was draining.
    let (mut document, expected_generation) =
        crate::cli::registry::fetch_versioned_document().await?;
    let removed = service::remove_service(&mut document, host, found.unit_id()).map_err(click)?;
    let directory_removed = document
        .get_mut("service_directory")
        .and_then(Value::as_object_mut)
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
        .is_some_and(|services| services.remove(name).is_some());
    if directory_removed {
        // A directory that cannot carry a counter is a document this command
        // did not write and must not silently repair; the removal still stands.
        let _ = crate::service_resolution::advance_generation(&mut document);
    }
    let generation =
        crate::cli::registry::push_document_if(&document, &expected_generation).await?;

    Ok(json!({
        "unit": removed.unit_id(),
        "host": host,
        "change": "removed",
        "directory_entry": if directory_removed { "removed" } else { "absent" },
        "registry_generation": generation,
    }))
}

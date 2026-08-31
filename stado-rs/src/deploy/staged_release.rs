//! Activate a release that is already staged on a host, using that release's
//! own installer.
//!
//! A managed host installs its own releases: a periodic unit runs the installer
//! that ships INSIDE the active release, which reads the deployment env file,
//! verifies the staged archive against the digest that file declares, unpacks
//! it and points the runtime at it. That works until the installer itself is
//! broken, and then it cannot be repaired by shipping a better one, because
//! the thing that would install the repair is the broken copy.
//!
//! charless-mac-mini spent a day in exactly that state: weles 0.5.40 shipped
//! `auto-deploy.sh` with a blank line inside a backslash continuation, so its
//! activator logged `syntax error near unexpected token '&&'` once per cycle
//! and installed nothing - including 0.5.43, which fixes that line and was
//! sitting in its local release root the whole time.
//!
//! This runs the STAGED archive's installer instead of the installed one, once.
//! Same env file, same digest contract, same script the host would have run
//! itself; the only difference is which copy of it executes. Two refusals guard
//! it: the staged archive must hash to the digest the coordinate declares, and
//! the installer must parse before it is run - the exact defect this exists to
//! escape should not be able to travel through it.
use super::{host_channel, service_env_file, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Where a host keeps staged release archives when the deployment env file
/// selects a local root rather than an API.
pub const LOCAL_ROOT_KEY: &str = "STADO_RELEASE_LOCAL_ROOT";

/// The installer every Weles release ships, relative to the archive root.
pub const INSTALLER_MEMBER: &str = "scripts/worker/deploy/auto-deploy.sh";

/// One product's coordinate, as the deployment env file declares it.
#[derive(Debug, PartialEq, Eq)]
pub struct Coordinate {
    pub version: String,
    pub sha256: String,
    pub local_root: String,
}

/// The env keys naming one product's staged release.
pub fn coordinate_keys(product: &str) -> (String, String) {
    let stem = product.replace('-', "_").to_uppercase();
    (
        format!("{stem}_RELEASE_VERSION"),
        format!("{stem}_RELEASE_SHA256"),
    )
}

/// Read one product's coordinate out of a deployment env file.
///
/// Last assignment wins, because the file is sourced top to bottom.
pub fn coordinate(body: &str, product: &str) -> Result<Coordinate, DeployError> {
    let (version_key, sha_key) = coordinate_keys(product);
    let mut version = None;
    let mut sha256 = None;
    let mut local_root = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let assignment = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        for (key, slot) in [
            (&version_key, &mut version),
            (&sha_key, &mut sha256),
            (&LOCAL_ROOT_KEY.to_string(), &mut local_root),
        ] {
            if let Some(value) = assignment.strip_prefix(&format!("{key}=")) {
                *slot = Some(service_env_file::effective_text(value).trim().to_string());
            }
        }
    }
    let missing = |key: &str| {
        DeployError(format!(
            "the deployment env file declares no {key}, so there is no staged release to activate"
        ))
    };
    let coordinate = Coordinate {
        version: version.filter(|v| !v.is_empty()).ok_or_else(|| missing(&version_key))?,
        sha256: sha256.filter(|v| !v.is_empty()).ok_or_else(|| missing(&sha_key))?,
        local_root: local_root
            .filter(|v| !v.is_empty())
            .ok_or_else(|| missing(LOCAL_ROOT_KEY))?,
    };
    if !coordinate
        .sha256
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        || coordinate.sha256.len() != 64
    {
        return Err(DeployError(format!(
            "{sha_key} is not a sha256 digest: {:?}",
            coordinate.sha256
        )));
    }
    Ok(coordinate)
}

/// The staged archive one coordinate names.
pub fn archive_path(coordinate: &Coordinate, product: &str, platform: &str) -> String {
    format!(
        "{}/{product}/{}/{platform}/{product}.tar.gz",
        coordinate.local_root.trim_end_matches('/'),
        coordinate.version
    )
}

/// Resolve `$HOME`, `${HOME}` or a leading `~` in a path the env file declares.
///
/// The file is written to be sourced, so its values carry shell variables. The
/// path goes into a quoted argument here, where nothing expands it, so it is
/// expanded once against the host's real home and any OTHER variable is
/// refused rather than shipped as a literal that silently matches nothing.
pub fn expand_home(path: &str, home: &str) -> Result<String, DeployError> {
    let home = home.trim_end_matches('/');
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.replace("${HOME}", home).replace("$HOME", home)
    };
    if expanded.contains('$') {
        return Err(DeployError(format!(
            "the deployment env file declares {path:?}, which this cannot resolve without \
             running a shell over it"
        )));
    }
    Ok(expanded)
}

/// Whether a staged archive is the one the coordinate declares.
///
/// The digest is the whole contract between what was delivered and what the
/// host agreed to run, so a mismatch refuses rather than installs.
pub fn digest_verdict(declared: &str, observed: &str) -> Result<(), DeployError> {
    if declared.eq_ignore_ascii_case(observed) {
        return Ok(());
    }
    Err(DeployError(format!(
        "the staged archive hashes to {observed}, but the deployment env file declares \
         {declared}; refusing to activate an archive the host has not agreed to run"
    )))
}

/// The first hex field of `shasum -a 256`.
pub fn parse_shasum(stdout: &str) -> Option<&str> {
    stdout
        .split_whitespace()
        .next()
        .filter(|field| field.len() == 64 && field.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// The script that unpacks the staged installer, checks it parses, and runs it
/// once.
///
/// `bash -n` first: this exists because an installer that could not be parsed
/// took a host's whole delivery path down, and running a second unparseable one
/// would repeat the outage rather than end it.
pub fn activation_script(archive: &str, version: &str) -> String {
    let archive = super::shlex_quote(archive);
    let version = super::shlex_quote(version);
    format!(
        r#"set -eu
archive={archive}
version={version}
work="$HOME/.stado/run/staged-release-activate"
mkdir -p "$work"
installer="$work/auto-deploy-$version.sh"
umask 077
tar -xzOf "$archive" ./{INSTALLER_MEMBER} > "$installer" 2>/dev/null \
  || tar -xzOf "$archive" {INSTALLER_MEMBER} > "$installer"
test -s "$installer" || {{ echo "STADO_ACTIVATE installer-missing"; exit 3; }}
bash -n "$installer" || {{ echo "STADO_ACTIVATE installer-unparseable"; exit 4; }}
echo "STADO_ACTIVATE installer-ready"
bash "$installer"
echo "STADO_ACTIVATE installer-exit=$?"
rm -f "$installer"
# The installer says why it did nothing only in its own log, which is far too
# large to fetch whole. Its last lines, and what the runtime link actually
# points at afterwards, are the report this verb owes its caller: a receipt
# written for one release while the link still names another is exactly the
# disagreement worth seeing.
tail -n 6 "$HOME/.local/state/weles/auto-deploy.log" 2>/dev/null | sed 's/^/STADO_ACTIVATE_LOG /' || true
printf 'STADO_ACTIVATE_LINK %s\n' "$(readlink "$HOME/weles" 2>/dev/null || echo not-a-symlink)"
# Activated is not the same as held. This host has a documented service that
# restores files it owns on every cycle, so the link is read again after it has
# had time to be taken back. A caller told "activated" about a link that was
# reverted thirty seconds later has been told nothing.
sleep 30
printf 'STADO_ACTIVATE_SETTLED %s\n' "$(readlink "$HOME/weles" 2>/dev/null || echo not-a-symlink)"
"#
    )
}

/// What one activation did, in the host's own words.
pub struct Activation {
    pub installed_version: String,
    pub api_before: bool,
    pub api_after: bool,
    pub log_tail: String,
}

/// Read the version the host is actually running now.
pub async fn installed_version(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let report = host_channel::run_script(
        target,
        "set -eu\nsed -n 's/.*\"version\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p' \
         \"$HOME/weles/package.json\" | head -1\n",
        runner,
    )
    .await?;
    Ok(report.stdout.trim().to_string())
}

/// Whether the worker API is answering on its port.
pub async fn api_answering(target: &ComputeTarget, port: u16, runner: &Runner) -> bool {
    host_channel::run_script(
        target,
        &format!(
            "curl -s -o /dev/null -m 5 http://127.0.0.1:{port}/healthz && echo up || echo down\n"
        ),
        runner,
    )
    .await
    .map(|report| report.stdout.contains("up"))
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV_FILE: &str = "export STADO_RELEASE_LOCAL_ROOT=$HOME/.stado/releases\n\
                            WELES_WORKER_RELEASE_VERSION=0.5.21\n\
                            # a later assignment wins, as a sourced file would\n\
                            WELES_WORKER_RELEASE_VERSION=\"0.5.43\"\n\
                            WELES_WORKER_RELEASE_SHA256=2714720eea1eaa430000000000000000000000000000000000000000000000ab\n";

    #[test]
    fn the_coordinate_is_the_last_assignment_the_env_file_makes() {
        let read = coordinate(ENV_FILE, "weles-worker").unwrap();
        assert_eq!(read.version, "0.5.43");
        assert_eq!(read.local_root, "$HOME/.stado/releases");
        assert_eq!(
            archive_path(&read, "weles-worker", "darwin-arm64"),
            "$HOME/.stado/releases/weles-worker/0.5.43/darwin-arm64/weles-worker.tar.gz"
        );
    }

    #[test]
    fn an_env_file_naming_no_staged_release_is_refused_by_key() {
        let said = coordinate("STADO_RELEASE_LOCAL_ROOT=/r\n", "weles-worker")
            .unwrap_err()
            .to_string();
        assert!(said.contains("WELES_WORKER_RELEASE_VERSION"), "{said}");
        let said = coordinate(
            "STADO_RELEASE_LOCAL_ROOT=/r\nWELES_WORKER_RELEASE_VERSION=0.5.43\n\
             WELES_WORKER_RELEASE_SHA256=nothex\n",
            "weles-worker",
        )
        .unwrap_err()
        .to_string();
        assert!(said.contains("is not a sha256 digest"), "{said}");
    }

    #[test]
    fn a_root_written_for_a_shell_is_resolved_once_against_the_real_home() {
        // charless-mac-mini's env file says exactly this, and a quoted argument
        // expands nothing - the first run of this verb looked for a directory
        // literally named $HOME.
        assert_eq!(
            expand_home("$HOME/.stado/releases/x.tar.gz", "/Users/charles"),
            Ok("/Users/charles/.stado/releases/x.tar.gz".to_string())
        );
        assert_eq!(
            expand_home("${HOME}/r", "/Users/charles/"),
            Ok("/Users/charles/r".to_string())
        );
        assert_eq!(expand_home("~/r", "/Users/charles"), Ok("/Users/charles/r".to_string()));
        let said = expand_home("$RELEASES/r", "/Users/charles").unwrap_err().to_string();
        assert!(said.contains("without running a shell over it"), "{said}");
    }

    #[test]
    fn an_archive_that_is_not_the_declared_one_refuses_before_anything_runs() {
        let declared = "a".repeat(64);
        let observed = "b".repeat(64);
        let said = digest_verdict(&declared, &observed).unwrap_err().to_string();
        assert!(said.contains("has not agreed to run"), "{said}");
        // Case is the only thing a host's tooling is allowed to differ on.
        digest_verdict(&declared, &declared.to_uppercase()).unwrap();
    }

    #[test]
    fn a_shasum_line_yields_only_a_real_digest() {
        assert_eq!(
            parse_shasum("2714720eea1eaa430000000000000000000000000000000000000000000000ab  /path\n"),
            Some("2714720eea1eaa430000000000000000000000000000000000000000000000ab")
        );
        assert_eq!(parse_shasum("shasum: no such file\n"), None);
        assert_eq!(parse_shasum(""), None);
    }

    #[test]
    fn the_activation_script_parse_checks_the_installer_before_running_it() {
        let script = activation_script("/r/weles-worker.tar.gz", "0.5.43");
        let parse_check = script.find("bash -n").expect("parse check");
        let run = script.find("bash \"$installer\"").expect("run");
        assert!(parse_check < run, "the parse check must come first:\n{script}");
        assert!(script.contains("installer-unparseable"), "{script}");
        // A path is one shell word on a real host. The payload may appear
        // inside the quoting - that is what quoting looks like - but it must
        // never begin a line, which is the only way it becomes a command.
        let script = activation_script("/r/x'; rm -rf ~; '.tar.gz", "0.5.43");
        assert!(
            !script
                .lines()
                .any(|line| line.trim_start().starts_with("rm -rf ~")),
            "{script}"
        );
        assert!(script.contains("archive='/r/x'"), "{script}");
    }
}

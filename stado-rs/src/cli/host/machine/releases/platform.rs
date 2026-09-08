use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

pub async fn verify_release_platform(
    target: &str,
    repo: &str,
    revision: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if !repo.starts_with("https://") {
        return Err(CmdError::click("--repo must be an https:// clone URL"));
    }
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CmdError::click("--ref must be a full lowercase Git commit"));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let repo = crate::deploy::shlex_quote(repo);
    let revision = crate::deploy::shlex_quote(revision);
    let script = format!(
        r#"set -euo pipefail
export PATH="$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"
root="$HOME/.stado/work"
/bin/mkdir -p "$root"
work=$(/usr/bin/mktemp -d "$root/release-platform.XXXXXX")
trap '/bin/rm -rf "$work"' EXIT HUP INT TERM
export TMPDIR="$work/tmp"
/bin/mkdir -p "$TMPDIR"
export TMP="$TMPDIR" TEMP="$TMPDIR"
export CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
/usr/bin/git -C "$work" init -q source
/usr/bin/git -C "$work/source" remote add origin {repo}
/usr/bin/git -C "$work/source" fetch -q --depth 1 origin {revision}
/usr/bin/git -C "$work/source" checkout -q --detach FETCH_HEAD
case "$(/usr/bin/uname -s):$(/usr/bin/uname -m)" in
  Darwin:arm64)
    platform=darwin-arm64
    digest=70c925dfe22be3f3c1879f94901977c583fa03b6f367583b4c93815e4ec8bde4
    ;;
  Linux:x86_64)
    platform=linux-amd64
    digest=4433afe3372d2c35cb33420307f5efe8b6e3b01bd7907b18d1d9c2b471f9ee68
    ;;
  *) printf 'unsupported native verification platform\n' >&2; exit 1 ;;
esac
/usr/bin/curl -fsSLo "$work/skarbiec.tar.gz" "https://github.com/wisent-ai/skarbiec/releases/download/v0.1.3/skarbiec-v0.1.3-$platform.tar.gz"
if [ "$platform" = darwin-arm64 ]; then
  printf '%s  %s\n' "$digest" "$work/skarbiec.tar.gz" | /usr/bin/shasum -a 256 -c -
else
  printf '%s  %s\n' "$digest" "$work/skarbiec.tar.gz" | /usr/bin/sha256sum -c -
fi
/bin/mkdir "$work/skarbiec"
/usr/bin/tar -xzf "$work/skarbiec.tar.gz" -C "$work/skarbiec"
export SKARBIEC_TEST_BIN="$work/skarbiec/skarbiec"
cd "$work/source/stado-rs"
cargo test --locked --test builds build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --test ci-cd a_real_release_builds_publishes_and_installs_its_binary -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --test ci-cd a_cancelled_release_build_is_retried_under_a_new_job -- --ignored --exact --nocapture --test-threads=1
"#
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(45 * 60),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        eprint!("{}", output.stderr);
        return Err(CmdError::click(format!(
            "{target}: platform verification failed:\n{}",
            output.stdout
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": target,
                "revision": revision.trim_matches('\''),
                "verified": true,
                "output": output.stdout,
                "stderr": output.stderr,
            }))?
        );
    } else {
        print!("{}", output.stdout);
    }
    Ok(())
}

/// `stado host build TARGET --manifest-path PATH --bin NAME [--json]` —
/// execute the one Cargo build Stado declares for a delivered source tree.
///
/// The variable inputs select a manifest and one binary; they never select a
/// shell command, Cargo flags, toolchain path, or working directory. Both the
/// lexical preflight and the host-side physical-path check bind the manifest
/// below the approved account's `$HOME/.stado/work/runs`.
pub async fn build(
    target: &str,
    manifest_path: &str,
    binary: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    crate::deploy::host_run::validate_run_descendant(manifest_path)
        .and_then(|_| crate::deploy::host_run::validate_binary_name(binary))
        .map_err(|error| CmdError::usage(error).machine_readable(json_output))?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let outcome = crate::deploy::host_run::build(
        &resolved,
        manifest_path,
        binary,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let exit_code = outcome.exit_code;
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    } else {
        print!("{}", outcome.stdout);
        eprint!("{}", outcome.stderr);
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(exit_code))
    }
}

/// `stado host run-attached TARGET --program PATH [--arg ARG]...` — attach
/// this process to one executable below the target account's managed run tree.
///
/// Standard input is inherited rather than read into a string, so a credential
/// body does not enter Stado's arguments, logs, or receipts. The non-JSON form
/// also inherits both output streams byte-for-byte. The JSON form captures
/// them only because a single machine-readable receipt cannot share stdout
/// with an arbitrary program protocol.
pub async fn run_attached(
    target: &str,
    program: &str,
    arguments: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    crate::deploy::host_run::validate_run_descendant(program)
        .and_then(|_| crate::deploy::host_run::validate_arguments(arguments))
        .map_err(|error| CmdError::usage(error).machine_readable(json_output))?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let outcome = crate::deploy::host_run::run_attached(&resolved, program, arguments, json_output)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let exit_code = outcome.exit_code;
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(exit_code))
    }
}

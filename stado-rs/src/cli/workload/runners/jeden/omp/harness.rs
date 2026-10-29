//! `stado workload harness install|status KIND --target HOST`: the harness a
//! session kind declares, brought to a host's managed path at the version
//! this machine runs.
//!
//! The version the operator runs here is the one a host receives, so a
//! session there behaves as the ones here do. A host on this machine's
//! platform receives this machine's copy; a host on another platform
//! fetches the build the harness's maker published for it (`releases` in
//! the declaration), checked against the release's own SHA-256 list. Either
//! lands in a managed run directory, is checked there by running it
//! (`--version` must answer what this machine's copy answers), and only then
//! replaces the managed program in one rename on the host's own volume.

use serde_json::json;

use crate::cli::workload::catalog::{WorkloadHarness, WorkloadKind};
use crate::cli::CmdError;
use crate::deploy::{host_channel, host_delivery, shlex_quote};
use crate::primitives::failure::FailureCode;

/// The managed run area deliveries are made into.
const RUN_AREA: &str = ".stado/work/runs";

/// The harness program on this machine's `PATH`, and what it says its
/// version is.
async fn local_harness(
    harness: &WorkloadHarness,
) -> Result<(std::path::PathBuf, String), CmdError> {
    let path = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(&harness.program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} is not on this machine's PATH, so there is no copy to carry to a host",
                harness.program
            ))
            .stating(FailureCode::NotFound)
        })?;
    let output =
        crate::wait::output_async(tokio::process::Command::new(&path).arg("--version")).await?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} --version failed here ({}): {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok((
        path,
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

/// What the managed harness on `target` answers to `--version`, or nothing
/// when it is not installed there.
async fn host_version(
    target: &crate::targets::ComputeTarget,
    managed: &str,
) -> Result<Option<String>, CmdError> {
    let script = format!(
        "program=\"$HOME\"/{}\n[ -x \"$program\" ] || exit 0\n\"$program\" --version\n",
        shlex_quote(managed)
    );
    let output = host_channel::run_script(target, &script, &crate::deploy::production_runner())
        .await
        .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "the managed harness on {} did not answer --version (exit {}): {}",
            target.name,
            output.code,
            output.detail().trim()
        )));
    }
    let version = output.stdout.trim().to_string();
    Ok(if version.is_empty() {
        None
    } else {
        Some(version)
    })
}

fn declared(kind: &WorkloadKind) -> Result<&WorkloadHarness, CmdError> {
    kind.harness.as_ref().ok_or_else(|| {
        CmdError::usage(format!(
            "workload kind '{}' runs no harness the fleet carries; its runtime is installed as \
             a product",
            kind.kind
        ))
    })
}

pub(crate) async fn status(
    kind: &WorkloadKind,
    target: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let harness = declared(kind)?;
    let (path, here) = local_harness(harness).await?;
    let target = crate::cli::canonical_host(target).await?;
    let there = host_version(&target, &harness.managed).await?;
    let current = there.as_deref() == Some(here.as_str());
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "kind": kind.kind,
            "target": target.name,
            "source": path,
            "version_here": here,
            "version_there": there,
            "managed": format!("~/{}", harness.managed),
            "current": current,
        }));
    } else {
        match there {
            Some(version) => println!(
                "{} on {}: {version} at ~/{} (this machine runs {here}{})",
                harness.program,
                target.name,
                harness.managed,
                if current {
                    ""
                } else {
                    "; install it with `stado workload harness install`"
                }
            ),
            None => println!(
                "{} is not installed on {}; install it with `stado workload harness install {} \
                 --target {}`",
                harness.program, target.name, kind.kind, target.name
            ),
        }
    }
    Ok(())
}

/// One published build of the harness: where the host fetches it and the
/// SHA-256 the release's checksum list gives it.
struct PublishedBuild {
    url: String,
    sha256: String,
}

/// The build of the version this machine runs (`version_line`, as
/// `--version` prints it) that the harness's maker publishes for
/// `target`'s platform, with its checksum read from the release's own list.
async fn published_build(
    harness: &WorkloadHarness,
    version_line: &str,
    target: &crate::targets::ComputeTarget,
    platform: &str,
) -> Result<PublishedBuild, CmdError> {
    let releases = harness.releases.as_ref().ok_or_else(|| {
        CmdError::refused(format!(
            "{} declares release platform {:?} and this machine's {} is built for {platform}, \
             and the harness declares no published builds in {}; install the harness from a {} \
             machine of the fleet",
            target.name,
            target.release_platform,
            harness.program,
            crate::cli::workload::catalog::DECLARATION_PATH,
            target.release_platform
        ))
    })?;
    let version = version_line
        .strip_prefix(&releases.version_prefix)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} --version answers {version_line:?} here, which does not carry a version \
                 after the declared prefix {:?}",
                harness.program, releases.version_prefix
            ))
            .stating(FailureCode::Config)
        })?;
    let asset = releases
        .assets
        .get(&target.release_platform)
        .ok_or_else(|| {
            CmdError::refused(format!(
                "{} publishes no build declared for {}'s release platform {:?}; declared: {}",
                harness.program,
                target.name,
                target.release_platform,
                releases
                    .assets
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    let address = |name: &str| {
        releases
            .url
            .replace("{version}", version)
            .replace("{asset}", name)
    };
    let checksums = address(&releases.checksums);
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| CmdError::click(crate::cli::http_failure(&error)))?;
    let unread = |cause: String| {
        CmdError::click(format!(
            "the checksum list of {} {version} at {checksums} could not be read: {cause}",
            harness.program
        ))
        .stating(FailureCode::InfraDown)
    };
    let response = crate::wait::request(client.get(&checksums))
        .await
        .map_err(|error| unread(crate::cli::http_failure(&error)))?;
    if !response.status().is_success() {
        return Err(unread(format!("HTTP {}", response.status())));
    }
    let listing = response
        .text()
        .await
        .map_err(|error| unread(crate::cli::http_failure(&error)))?;
    let sha256 = listing
        .lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            let sum = words.next()?;
            let name = words.next()?.trim_start_matches('*');
            (name == asset).then(|| sum.to_ascii_lowercase())
        })
        .filter(|sum| sum.len() == 64 && sum.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| unread(format!("it gives no SHA-256 for {asset}")))?;
    Ok(PublishedBuild {
        url: address(asset),
        sha256,
    })
}

pub(crate) async fn install(
    kind: &WorkloadKind,
    target: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let harness = declared(kind)?;
    let (path, here) = local_harness(harness).await?;
    let target = crate::cli::canonical_host(target).await?;
    // A host on this machine's platform receives this machine's program; one
    // on another platform could not run it, and receives the same version as
    // the harness's maker built it for that platform.
    let platform =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .map_err(CmdError::refused)?;
    let runner = crate::deploy::production_runner();
    let previous = host_version(&target, &harness.managed).await?;
    let mut source = None;
    if previous.as_deref() != Some(here.as_str()) {
        let run_id = uuid::Uuid::new_v4().to_string();
        let run = format!("{RUN_AREA}/{run_id}");
        let delivered = format!("{run}/{}", harness.program);
        let fetch = if target.release_platform == platform {
            host_delivery::deliver_host(
                &target.name,
                &path.to_string_lossy(),
                &delivered,
                None,
                &runner,
            )
            .await
            .map_err(CmdError::from)?;
            source = Some(path.display().to_string());
            String::new()
        } else {
            let build = published_build(harness, &here, &target, platform).await?;
            let fetch = format!(
                "command -v curl >/dev/null || {{ printf 'curl is not on this host, so the published harness cannot be fetched\\n' >&2; exit 1; }}\n\
                 mkdir -p \"${{staged%/*}}\" || exit\n\
                 curl --fail --location --silent --show-error --output \"$staged\" {url} || exit\n\
                 if command -v sha256sum >/dev/null; then sum=$(sha256sum \"$staged\") || exit; else sum=$(shasum -a 256 \"$staged\") || exit; fi\n\
                 sum=${{sum%% *}}\n\
                 if [ \"$sum\" != {sha256} ]; then printf 'the fetched harness hashes to %s, not the published %s\\n' \"$sum\" {sha256} >&2; exit 1; fi\n\
                 chmod 0755 \"$staged\" || exit\n",
                url = shlex_quote(&build.url),
                sha256 = shlex_quote(&build.sha256),
            );
            source = Some(build.url);
            fetch
        };
        let script = format!(
            "set -u\nhome=${{HOME%/}}\nstaged=\"$home\"/{staged}\nmanaged=\"$home\"/{managed}\n\
             run=\"$home\"/{run}\ntrap 'rm -rf \"$run\"' EXIT\n\
             {fetch}\
             answer=$(\"$staged\" --version) || exit\n\
             if [ \"$answer\" != {expected} ]; then printf 'the delivered harness answers %s, not %s\\n' \"$answer\" {expected} >&2; exit 1; fi\n\
             mkdir -p \"${{managed%/*}}\" || exit\n\
             mv -f \"$staged\" \"$managed\" || exit\n",
            staged = shlex_quote(&delivered),
            managed = shlex_quote(&harness.managed),
            expected = shlex_quote(&here),
            run = shlex_quote(&run),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(CmdError::from)?;
        if !output.succeeded() {
            return Err(CmdError::click(format!(
                "{} from {} was not installed at ~/{} on {} (exit {}): {}",
                harness.program,
                source.as_deref().unwrap_or("this machine"),
                harness.managed,
                target.name,
                output.code,
                output.detail().trim()
            )));
        }
    }
    let installed = host_version(&target, &harness.managed).await?;
    if installed.as_deref() != Some(here.as_str()) {
        return Err(CmdError::click(format!(
            "~/{} on {} answers {installed:?} after the install, not {here}",
            harness.managed, target.name
        )));
    }
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "kind": kind.kind,
            "target": target.name,
            "version": here,
            "previous": previous,
            "managed": format!("~/{}", harness.managed),
            "changed": source.is_some(),
            "source": source,
        }));
    } else if let Some(source) = &source {
        println!(
            "{} {here} installed at ~/{} on {} from {source}",
            harness.program, harness.managed, target.name
        );
    } else {
        println!("{} on {} already runs {here}", harness.program, target.name);
    }
    Ok(())
}

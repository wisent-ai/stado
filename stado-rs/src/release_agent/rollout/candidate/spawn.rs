//! Start one candidate on its own port, and ask it whether it is ready yet.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::Utc;

use crate::release_agent::rollout::serving::discover::pid_alive;
use crate::release_agent::state::evidence::release_log;
use crate::release_agent::state::records::ProcessRecord;
use crate::release_control::{self, ProductReleasePolicy, ReleaseManifest, ReleaseTargetPolicy};

fn expand_home(value: &str, home: &str) -> String {
    value.replace("{home}", home)
}

pub(crate) fn spawn_release(
    product: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    manifest: &ReleaseManifest,
    release_dir: &Path,
    port: u16,
) -> Result<ProcessRecord, String> {
    let launcher = release_dir.join(&policy.launcher);
    let binary = release_dir.join(&policy.binary);
    for path in [&launcher, &binary] {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("release entry {} is unavailable: {error}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "release entry is not a regular file: {}",
                path.display()
            ));
        }
    }
    let runtime = Path::new(&target.runtime_root)
        .join(product)
        .join(format!("{}-{port}", manifest.version));
    std::fs::create_dir_all(&runtime).map_err(|error| {
        format!(
            "cannot create candidate runtime {}: {error}",
            runtime.display()
        )
    })?;
    let stdout = release_log(target, product, &manifest.version, "out")?;
    let stderr = release_log(target, product, &manifest.version, "err")?;
    let mut command = Command::new("/usr/bin/sudo");
    command
        .args(["-n", "-u", &target.run_as_user, "-H", "/usr/bin/env"])
        .arg(format!("HOME={}", target.home))
        // `sudo` replaces PATH with its own `secure_path`, which carries no
        // Homebrew prefix, and a candidate started with that PATH cannot find
        // the helpers its product shells out to. `ask_wall` already sets this
        // exact list for the same reason -- "the decrypt helper lives there,
        // and without it every answer would be an unreachable one" -- and the
        // managed launchd units this rollout replaces carry it too, so a
        // candidate without it is the only shape of the process that has ever
        // run without a PATH.
        //
        // It cost this fleet three days. Every skarbiec candidate from
        // 2026-09-01 onward failed readiness with `stored item cannot be
        // decrypted: spawn gpg: No such file or directory`, was quarantined,
        // and left the vault unreadable; the object plane's verifiers read
        // Skarbiec, so the whole control plane answered `503 object
        // authorization unavailable`, and every Brama agent identity 401'd
        // behind it. The launchd unit had the PATH, the rollout did not, and
        // nothing compared the two declarations.
        //
        // `policy.environment` is applied after this, so a product that needs
        // a different PATH still declares one and wins.
        .arg("PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        .arg(format!("STADO_RELEASE_PRODUCT={product}"))
        .arg(format!("STADO_RELEASE_VERSION={}", manifest.version))
        .arg(format!("STADO_RELEASE_PLATFORM={}", manifest.platform))
        .arg(format!("STADO_RELEASE_SHA256={}", manifest.artifact_sha256))
        .arg(format!("{}={}", policy.binary_env, binary.display()))
        .arg(format!("{}={port}", policy.port_env))
        .arg(format!("{}={}", policy.runtime_env, runtime.display()));
    for (name, value) in &policy.environment {
        command.arg(format!("{name}={}", expand_home(value, &target.home)));
    }
    let child = command
        .arg(&launcher)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            format!(
                "cannot start {product} {} candidate: {error}",
                manifest.version
            )
        })?;
    Ok(ProcessRecord {
        version: manifest.version.clone(),
        artifact_sha256: manifest.artifact_sha256.clone(),
        manifest_sha256: release_control::sha256_bytes(&release_control::canonical_manifest(
            manifest,
        )?),
        port,
        pid: child.id() as i32,
        release_dir: release_dir.display().to_string(),
        started_at: Utc::now(),
    })
}

/// Why a candidate is not ready yet, or `None` when it is.
///
/// This used to be a `bool`, and the quarantine record it fed said only
/// "candidate did not become ready before deadline". A brama candidate was
/// quarantined four times across twelve days with its own log showing
/// `Starting brama server on 127.0.0.1:18080` seconds earlier, and the record
/// could not distinguish a dead process from a refused connection from an HTTP
/// status. Every hypothesis had to be excluded by reading the product's source.
pub(crate) async fn not_ready_because(record: &ProcessRecord, path: &str) -> Option<String> {
    if !pid_alive(record.pid) {
        return Some(format!("pid {} is gone", record.pid));
    }
    let url = format!("http://127.0.0.1:{}{}", record.port, path);
    match reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(format!("{url} answered HTTP {}", response.status())),
        Err(error) if error.is_timeout() => Some(format!("{url} did not answer within 3s")),
        Err(error) if error.is_connect() => Some(format!("{url} refused the connection")),
        Err(error) => Some(format!("{url} failed: {error}")),
    }
}

/// Wait for readiness, returning the last reason it was refused.
pub(crate) async fn await_ready_because(
    record: &ProcessRecord,
    readiness_path: &str,
    seconds: u64,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let reason = match not_ready_because(record, readiness_path).await {
            None => return None,
            Some(reason) => reason,
        };
        if tokio::time::Instant::now() >= deadline {
            return Some(reason);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

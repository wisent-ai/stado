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

/// How long a release that is already serving may keep refusing its readiness
/// probe before the agent treats it as lost. One refused probe is a busy host;
/// half a minute of them is a release that is not serving.
pub(crate) const LOST_READINESS_CONFIRMATION_SECONDS: u64 = 30;

/// Why a release that was serving is no longer ready, confirmed over a window,
/// or `None` when it answers.
///
/// One refused probe is not a lost release. On 2026-09-19 brama 0.4.41 was
/// rolled back and quarantined on charless-mac-mini for `did not answer within
/// 3s` while its process was alive and its own log, seconds either side, shows
/// it working through a model-discovery sweep on a host running hundreds of
/// jobs. A release that is really gone stays gone, so the verdict is confirmed
/// before it costs a rollback. A process that has exited is reported at once:
/// there is nothing to wait for, and holding a rollback for half a minute over
/// a pid that is already gone is time the fleet spends serving nothing.
pub(crate) async fn lost_readiness_because(
    record: &ProcessRecord,
    readiness_path: &str,
) -> Option<String> {
    lost_readiness_within(record, readiness_path, LOST_READINESS_CONFIRMATION_SECONDS).await
}

async fn lost_readiness_within(
    record: &ProcessRecord,
    readiness_path: &str,
    seconds: u64,
) -> Option<String> {
    let first = not_ready_because(record, readiness_path).await?;
    if !pid_alive(record.pid) {
        return Some(first);
    }
    let confirmed = await_ready_because(record, readiness_path, seconds).await?;
    Some(format!(
        "{confirmed}, for {seconds}s (first refusal: {first})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A loopback server that hangs up on its first `refusals` connections and
    /// answers 200 after that. Returns the port it listens on.
    async fn flaky_readiness(refusals: usize) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("bound address").port();
        let seen = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut discard = [0_u8; 1024];
                    let _ = socket.read(&mut discard).await;
                    if seen.fetch_add(1, Ordering::SeqCst) < refusals {
                        return;
                    }
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await;
                    let _ = socket.flush().await;
                });
            }
        });
        port
    }

    fn record(port: u16, pid: i32) -> ProcessRecord {
        ProcessRecord {
            version: "0.0.0-test".into(),
            artifact_sha256: String::new(),
            manifest_sha256: String::new(),
            port,
            pid,
            release_dir: String::new(),
            started_at: Utc::now(),
        }
    }

    /// The 2026-09-19 incident: brama 0.4.41 refused one probe while it was
    /// alive and busy, and was rolled back and quarantined for it.
    #[tokio::test]
    async fn a_release_that_refuses_once_and_then_answers_is_not_lost() {
        let port = flaky_readiness(1).await;
        let record = record(port, std::process::id() as i32);
        assert!(
            not_ready_because(&record, "/readyz").await.is_some(),
            "the first probe is refused, which is what starts the confirmation"
        );
        assert_eq!(lost_readiness_within(&record, "/readyz", 5).await, None);
    }

    /// A release that never answers is lost, and the verdict carries both the
    /// first refusal and the confirmed one.
    #[tokio::test]
    async fn a_release_that_never_answers_within_the_window_is_lost() {
        let port = flaky_readiness(usize::MAX).await;
        let record = record(port, std::process::id() as i32);
        let why = lost_readiness_within(&record, "/readyz", 1)
            .await
            .expect("a release that never answers is lost");
        assert!(why.contains("for 1s"), "{why}");
        assert!(why.contains("first refusal"), "{why}");
    }

    /// A process that is gone is reported without waiting out the window.
    #[tokio::test]
    async fn a_dead_process_is_reported_immediately() {
        let port = flaky_readiness(0).await;
        let started = std::time::Instant::now();
        let why = lost_readiness_within(&record(port, i32::MAX), "/readyz", 30)
            .await
            .expect("a dead process is lost");
        assert!(why.contains("is gone"), "{why}");
        assert!(started.elapsed() < Duration::from_secs(5), "{why}");
    }
}

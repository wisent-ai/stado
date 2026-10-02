//! Start one candidate on its own port, and ask it whether it is ready yet.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use chrono::Utc;

use crate::release_agent::rollout::processes::inventory::listener_pid;
use crate::release_agent::rollout::serving::discover::pid_alive;
use crate::release_agent::state::evidence::release_log;
use crate::release_agent::state::records::ProcessRecord;
use crate::release_cause::{QuarantineCause, Refusal};
use crate::release_control::{
    self, ProductReleasePolicy, ReleaseManifest, ReleaseTargetPolicy, RolloutStrategy,
};

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
        // A candidate spawned without it fails readiness with `stored item
        // cannot be decrypted: spawn gpg: No such file or directory`, is
        // quarantined, and leaves the vault unreadable; the object plane's
        // verifiers read Skarbiec, so the whole control plane then answers
        // `503 object authorization unavailable`, and every Brama agent
        // identity 401s behind it — the launchd unit has the PATH, the rollout
        // does not, and nothing compares the two declarations.
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
///
/// The cause is named here, where it is observed: a pid that is gone while
/// another process listens on the candidate's port is a port somebody else
/// holds; a pid that is gone with the port free is a process that vanished;
/// a probe the HTTP client gave up on is a probe nobody answered.
pub(crate) async fn not_ready_because(record: &ProcessRecord, path: &str) -> Option<Refusal> {
    if !pid_alive(record.pid) {
        let holder = listener_pid(Some(record.port)).filter(|pid| *pid != record.pid);
        return Some(match holder {
            Some(holder) => Refusal::observed(
                QuarantineCause::StableBindOccupied,
                format!(
                    "pid {} is gone while pid {holder} listens on its port {}",
                    record.pid, record.port
                ),
            ),
            None => Refusal::observed(
                QuarantineCause::ReleaseProcessVanished,
                format!("pid {} is gone", record.pid),
            ),
        });
    }
    let url = format!("http://127.0.0.1:{}{}", record.port, path);
    // The probe waits for the candidate's answer: a release working through a
    // long sweep is slow, not lost, and only its own answer says which.
    match reqwest::Client::new().get(&url).send().await {
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(format!("{url} answered HTTP {}", response.status()).into()),
        Err(error) if error.is_connect() => Some(format!("{url} refused the connection").into()),
        Err(error) if error.is_timeout() => Some(Refusal::observed(
            QuarantineCause::ReadinessProbeUnanswered,
            format!("{url} did not answer: {error}"),
        )),
        Err(error) => Some(format!("{url} failed: {error}").into()),
    }
}

/// One readiness read of a release, judged against the readiness window its
/// release policy declares.
pub(crate) enum Readiness {
    /// It answered.
    Ready,
    /// It did not answer, its process is alive and the window is still open:
    /// a later pass reads it again.
    Pending(Refusal),
    /// Its process is gone, or it still did not answer once the window was
    /// over.
    Failed(Refusal),
}

/// Read `record`'s readiness once. `elapsed_seconds` is how long the window
/// has been open, counted by the caller from the moment readiness became
/// owed (a candidate's start, the end of a drain).
///
/// One refused probe is not a lost release: a release can be alive and
/// working through a long sweep on a busy host, so a live process is given
/// the policy's whole window, read once per pass. A process that has exited
/// is reported at once: there is nothing to read again.
pub(crate) async fn readiness(
    record: &ProcessRecord,
    readiness_path: &str,
    strategy: &RolloutStrategy,
    elapsed_seconds: u64,
) -> Readiness {
    let Some(why) = not_ready_because(record, readiness_path).await else {
        return Readiness::Ready;
    };
    if !pid_alive(record.pid) {
        return Readiness::Failed(why);
    }
    let window = strategy.readiness_timeout_seconds;
    if elapsed_seconds >= window {
        return Readiness::Failed(
            why.context(|said| format!("{said}, still after the {window}s readiness window")),
        );
    }
    Readiness::Pending(why)
}

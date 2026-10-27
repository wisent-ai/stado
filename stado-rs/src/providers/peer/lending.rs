//! One lending session: the Stado worker this host runs against a peer's
//! queue, started, drained and ended.
//!
//! The worker is this same Stado program, `stado serve --worker
//! --standalone --kind stado-peer --idle-shutdown`, started as its own
//! process with a home of its own under `~/.stado/market/stado/<peer>/homes/`
//! and an environment that holds nothing of ours but `PATH`. Its
//! configuration names only the peer's store, so it reads no registry, no
//! vault and no queue of this fleet; its role lock, its run directories and
//! its job work trees live in that home. It ends by itself once the peer's
//! queue holds nothing it can run. To withdraw it, the loop asks it to drain
//! ([`crate::providers::local::agent::drain`]): it claims nothing more, the
//! peer's jobs it runs finish and report to the peer, and then it exits.
//! Ending the process instead would leave those jobs running in their own
//! process groups with nobody renewing their leases.
//!
//! When it has exited its log is kept beside the peer's other logs and its
//! home is removed, so a session leaves no work tree behind.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::remote::{install_bearer, peer_directory};
use super::{PeerDeclaration, PeerError, Session, LENDING_KIND};

/// A lending worker this process started and waits for.
pub struct Lending {
    child: tokio::process::Child,
    home: PathBuf,
    log: PathBuf,
    /// The session's start, as its home and its kept log are named.
    stamp: String,
    pub session: Session,
}

fn local(what: &str, path: &Path, error: std::io::Error) -> PeerError {
    PeerError::Local(format!("{what} {}: {error}", path.display()))
}

/// The directory a peer's ended sessions keep their worker logs in.
pub fn log_directory(peer: &str) -> PathBuf {
    peer_directory(peer).join("logs")
}

/// Write the worker's configuration and log into `home` and start it.
fn launch(
    peer: &PeerDeclaration,
    token_file: &Path,
    home: &Path,
    poll_seconds: u64,
) -> Result<(tokio::process::Child, PathBuf), PeerError> {
    let state = home.join(".stado");
    let mut stado = json!({
        "url": peer.origin,
        "token_file": token_file.to_string_lossy(),
        "namespace": peer.namespace,
    });
    if let Some(ca_file) = &peer.ca_file {
        stado["ca_file"] = json!(ca_file);
    }
    let config = state.join("config.json");
    let document = json!({"storage": {"backend": "stado", "stado": stado}});
    let body = serde_json::to_string_pretty(&document).map_err(|error| {
        PeerError::Local(format!("the worker configuration cannot be encoded: {error}"))
    })?;
    std::fs::write(&config, body)
        .map_err(|error| local("cannot write the worker configuration", &config, error))?;
    let log = home.join("worker.log");
    let output = std::fs::File::create(&log)
        .map_err(|error| local("cannot create the worker log", &log, error))?;
    let errors = output
        .try_clone()
        .map_err(|error| local("cannot share the worker log", &log, error))?;
    let program = std::env::current_exe()
        .map_err(|error| PeerError::Local(format!("this program's path cannot be read: {error}")))?;
    let mut command = tokio::process::Command::new(&program);
    command
        .args([
            "serve",
            "--worker",
            "--standalone",
            "--kind",
            LENDING_KIND,
            "--idle-shutdown",
            "--poll-seconds",
            &poll_seconds.to_string(),
        ])
        .env_clear()
        .env("HOME", home)
        .env("STADO_CONFIG", &config)
        .envs(std::env::var_os("PATH").map(|path| ("PATH", path)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(output))
        .stderr(std::process::Stdio::from(errors));
    let child = command.spawn().map_err(|error| {
        PeerError::Local(format!(
            "the lending worker {} could not be started: {error}",
            program.display()
        ))
    })?;
    Ok((child, log))
}

impl Lending {
    /// Start a lending worker for `peer`, polling at the loop's own period.
    pub async fn start(
        peer: &PeerDeclaration,
        poll_seconds: u64,
        max_duration_s: Option<i64>,
    ) -> Result<Self, PeerError> {
        let token_file = install_bearer(peer).await?;
        let started = chrono::Utc::now();
        let stamp = started.format("%Y%m%dT%H%M%S%.6fZ").to_string();
        let home = peer_directory(&peer.name).join("homes").join(&stamp);
        let state = home.join(".stado");
        std::fs::create_dir_all(&state)
            .map_err(|error| local("cannot create the lending home", &state, error))?;
        let launched = launch(peer, &token_file, &home, poll_seconds);
        let (child, log) = match launched {
            Ok(launched) => launched,
            // A start that failed leaves no home behind; a home that cannot
            // be removed is said beside the failure.
            Err(error) => {
                return Err(match std::fs::remove_dir_all(&home) {
                    Ok(()) => error,
                    Err(cleanup) => PeerError::Local(format!(
                        "{error}; the lending home {} could not be removed: {cleanup}",
                        home.display()
                    )),
                })
            }
        };
        let Some(pid) = child.id() else {
            return Err(PeerError::Local(format!(
                "the lending worker exited before its process id could be read; its log is {}",
                log.display()
            )));
        };
        let session = Session {
            started_at: started.to_rfc3339(),
            pid,
            home: home.to_string_lossy().into_owned(),
            max_duration_s,
            ended_at: None,
            end: None,
        };
        Ok(Self {
            child,
            home,
            log,
            stamp,
            session,
        })
    }

    /// Seconds the session has run.
    pub fn seconds_running(&self) -> Result<i64, PeerError> {
        let started = chrono::DateTime::parse_from_rfc3339(&self.session.started_at).map_err(
            |error| {
                PeerError::Local(format!(
                    "the session start {:?} cannot be read back: {error}",
                    self.session.started_at
                ))
            },
        )?;
        Ok((chrono::Utc::now() - started.with_timezone(&chrono::Utc)).num_seconds())
    }

    /// Ask the worker to claim nothing more and exit once its jobs finish.
    pub fn drain(&self, reason: &str) -> Result<(), PeerError> {
        crate::providers::local::agent::drain::request(&self.home, reason)
            .map_err(|error| local("cannot ask the lending worker to drain in", &self.home, error))
    }

    /// How the worker ended, once it has; `None` while it runs.
    pub fn exited(&mut self) -> Result<Option<String>, PeerError> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(format!("the lending worker exited with {status}"))),
            Ok(None) => Ok(None),
            Err(error) => Err(PeerError::Local(format!(
                "whether the lending worker (pid {}) still runs cannot be read: {error}",
                self.session.pid
            ))),
        }
    }

    /// Record the end, keep the worker's log, and remove its home. A cleanup
    /// step that fails is said in the end itself, never dropped.
    pub fn finish(self, peer: &str, end: String) -> Session {
        let mut end = end;
        let logs = log_directory(peer);
        let kept = logs.join(format!("{}.log", self.stamp));
        let kept_log =
            std::fs::create_dir_all(&logs).and_then(|()| std::fs::rename(&self.log, &kept));
        match kept_log {
            Ok(()) => end.push_str(&format!("; its log is {}", kept.display())),
            Err(error) => end.push_str(&format!(
                "; its log {} could not be kept: {error}",
                self.log.display()
            )),
        }
        if let Err(error) = std::fs::remove_dir_all(&self.home) {
            end.push_str(&format!(
                "; its home {} could not be removed: {error}",
                self.home.display()
            ));
        }
        Session {
            ended_at: Some(chrono::Utc::now().to_rfc3339()),
            end: Some(end),
            ..self.session
        }
    }
}

/// Ask a worker this process did not start — one a previous loop left
/// running — to drain, by its recorded home.
pub fn drain_left_behind(session: &Session, reason: &str) -> Result<(), PeerError> {
    let home = PathBuf::from(&session.home);
    crate::providers::local::agent::drain::request(&home, reason)
        .map_err(|error| local("cannot ask the lending worker to drain in", &home, error))
}

//! A request's consultation of the vault for a bearer, and what a request
//! that would stand behind one is answered instead.
//!
//! Every bearer the listener compares a request against comes from Skarbiec:
//! a namespace bearer's version on every object request, the bearer itself
//! when the version moved, the release publisher's and the host-health role's
//! on every request of theirs. While the vault's key database is held by a
//! process that never lets go, each of those reads waits until gpg gives up —
//! tens of minutes — and answers 503. Requests used to queue behind that read
//! on this listener's own lock and each then paid the same wait: the object
//! API held every request of the fleet for hours and answered none.
//!
//! A consultation is now one of two shapes. A shared one (a version read, a
//! per-request bearer read) runs beside its peers, as it always did. An
//! exclusive one (a namespace bearer read, which the vault audits and decrypts)
//! holds its key, so one runs at a time. A request that finds its key held is
//! not queued: it is refused at once, carrying the holder's `czekam` line, so
//! the caller reads what the server waits on, where and since when. Once a
//! consultation of a key has failed, every later one is refused with that
//! failure's `blad czekania` line, except the first, which consults the vault
//! again, holding the key while it does; its success clears the failure. No
//! timer decides any of this: the vault's answers do.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::future::Future;

use serde_json::{json, Value};

use crate::dashboard::listener::Dashboard;
use crate::skarbiec::SkarbiecError;

/// What the listener knows about its consultations of the vault under one
/// key: an object namespace, a release publisher item or the host-health
/// role.
#[derive(Clone, Default)]
pub(crate) struct VaultConsultation {
    /// The `czekam` line of the consultation holding this key now.
    pub(crate) open: Option<String>,
    /// The `blad czekania` line of the last consultation of this key that
    /// failed, until one succeeds.
    pub(crate) failed: Option<String>,
}

/// One run of the declared crypto repair this listener started because a
/// consultation's failure named the process holding the vault's keyring
/// lock.
#[derive(Clone)]
pub(crate) struct KeyringLockRepair {
    /// The vault's own sentence naming the holder.
    pub(crate) holder: String,
    /// When the repair started, RFC 3339 UTC.
    pub(crate) started: String,
    /// How it ended, or `None` while it runs.
    pub(crate) outcome: Option<String>,
}

/// Every key's consultation state and the keyring-lock repair, for
/// `/api/state.json`.
#[derive(Default)]
pub(crate) struct VaultConsultations {
    keys: BTreeMap<String, VaultConsultation>,
    repair: Option<KeyringLockRepair>,
}

impl VaultConsultations {
    fn entry(&mut self, key: &str) -> &mut VaultConsultation {
        self.keys.entry(key.to_string()).or_default()
    }

    /// Each key with the line it is held by and the line it last failed
    /// with, and the keyring-lock repair this listener last ran: the
    /// operator's read of what the listener stands on.
    pub(crate) fn state_json(&self) -> Value {
        let consultations: serde_json::Map<String, Value> = self
            .keys
            .iter()
            .map(|(key, consultation)| {
                (
                    key.clone(),
                    json!({
                        "open": consultation.open,
                        "failed": consultation.failed,
                    }),
                )
            })
            .collect();
        json!({
            "consultations": consultations,
            "keyring_lock_repair": self.repair.as_ref().map(|repair| json!({
                "holder": repair.holder,
                "started": repair.started,
                "outcome": repair.outcome,
            })),
        })
    }
}

/// Why a request could not have its bearer compared: the authority was not
/// consulted. `cause` is the server's own wait line — the `czekam` line of
/// the consultation this request would have stood behind, or the `blad
/// czekania` line of the one that failed — or, for a fault before any wait,
/// the fault's own sentence.
#[derive(Debug, Clone)]
pub(crate) struct AuthorityUnavailable {
    pub(crate) cause: String,
}

impl AuthorityUnavailable {
    pub(crate) fn new(cause: impl Display) -> Self {
        Self {
            cause: cause.to_string(),
        }
    }

    /// The body every route answers with when the authority could not be
    /// consulted: the refusal the fleet has always read, and beside it the
    /// cause in the server's own words.
    pub(crate) fn body(&self, what: &str) -> Value {
        json!({
            "error": format!("{what} authorization unavailable"),
            "cause": self.cause,
        })
    }
}

/// Holds a key from the moment a consultation claims it until its outcome is
/// recorded. Dropped before that — the request's future was dropped with the
/// client gone — it releases the key so the next request can consult.
struct Claim<'a> {
    dashboard: &'a Dashboard,
    key: &'a str,
    settled: bool,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.dashboard.vault_release(self.key);
        }
    }
}

impl Dashboard {
    fn vault_release(&self, key: &str) {
        self.vault
            .write()
            .expect("dashboard vault consultation lock")
            .entry(key)
            .open = None;
    }

    /// Consult the vault once under `key` for `what`, saying so on stderr
    /// like every wait, under the rules in the module's own words: refused
    /// at once when the key is held or when its last consultation failed and
    /// another request is already consulting again; `exclusive` holds the
    /// key for the length of `work`.
    pub(crate) async fn consult_vault<T, F>(
        &self,
        key: &str,
        exclusive: bool,
        what: impl Display,
        work: F,
    ) -> Result<T, AuthorityUnavailable>
    where
        F: Future<Output = Result<T, SkarbiecError>>,
    {
        let waiting = crate::wait::begin(crate::wait::Kind::Network, what, "Skarbiec");
        let claimed = {
            let mut consultations = self
                .vault
                .write()
                .expect("dashboard vault consultation lock");
            let consultation = consultations.entry(key);
            if let Some(open) = &consultation.open {
                let cause = open.clone();
                drop(consultations);
                waiting.failed(format!("stands behind another consultation: {cause}"));
                return Err(AuthorityUnavailable::new(cause));
            }
            if exclusive || consultation.failed.is_some() {
                consultation.open = Some(waiting.open_line());
                true
            } else {
                false
            }
        };
        let mut claim = claimed.then(|| Claim {
            dashboard: self,
            key,
            settled: false,
        });
        let outcome = work.await;
        let mut consultations = self
            .vault
            .write()
            .expect("dashboard vault consultation lock");
        let consultation = consultations.entry(key);
        if let Some(claim) = claim.as_mut() {
            consultation.open = None;
            claim.settled = true;
        }
        match outcome {
            Ok(value) => {
                consultation.failed = None;
                drop(consultations);
                waiting.done();
                Ok(value)
            }
            Err(error) => {
                let holder = error.keyring_lock_holder();
                let cause = crate::wait::chain(&error);
                let line = waiting.failure_line(&cause);
                consultation.failed = Some(line.clone());
                drop(consultations);
                waiting.failed(cause);
                if let Some(holder) = holder {
                    self.repair_keyring_lock(holder);
                }
                Err(AuthorityUnavailable::new(line))
            }
        }
    }

    /// A consultation's failure named the process holding the vault's keyring
    /// lock. On the host that declares the vault, run the declared crypto
    /// repair (`skarbiec recover-daemons`, the same payload `stado repair
    /// skarbiec --step crypto` runs) once for this holder, in the background,
    /// and keep its outcome for `/api/state.json`; one repair runs at a time,
    /// and the same holder is not acted on twice, so a repair that could not
    /// release the lock is read, not repeated. On any other host the repair
    /// belongs to the vault host and the log says which command runs it.
    fn repair_keyring_lock(&self, holder: String) {
        if crate::config::skarbiec_vault_file().is_empty() {
            eprintln!(
                "[dashboard] the vault at {} names a held keyring lock ({holder}); this host \
                 declares no vault, so the repair runs on the vault host: `stado repair skarbiec \
                 --step crypto --target <vault host> --apply` or `stado host exec <vault host> \
                 -- skarbiec recover-daemons`",
                crate::config::skarbiec_url()
            );
            return;
        }
        {
            let mut consultations = self
                .vault
                .write()
                .expect("dashboard vault consultation lock");
            if let Some(repair) = &consultations.repair {
                match &repair.outcome {
                    None => {
                        eprintln!(
                            "[dashboard] keyring lock repair for {} is still running since {}; \
                             not started again for {holder}",
                            repair.holder, repair.started
                        );
                        return;
                    }
                    Some(outcome) if repair.holder == holder => {
                        eprintln!(
                            "[dashboard] keyring lock repair already ran for {holder} at {} and \
                             ended: {outcome}; not repeated",
                            repair.started
                        );
                        return;
                    }
                    Some(_) => {}
                }
            }
            consultations.repair = Some(KeyringLockRepair {
                holder: holder.clone(),
                started: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                outcome: None,
            });
        }
        eprintln!("[dashboard] the vault names a held keyring lock ({holder}); running the declared skarbiec crypto repair on this host");
        let vault = self.vault.clone();
        tokio::spawn(async move {
            let outcome = crypto_repair_outcome().await;
            eprintln!("[dashboard] skarbiec crypto repair for {holder}: {outcome}");
            if let Some(repair) = vault
                .write()
                .expect("dashboard vault consultation lock")
                .repair
                .as_mut()
            {
                repair.outcome = Some(outcome);
            }
        });
    }
}

/// Run the declared skarbiec crypto repair payload on this host, the readiness
/// it probes being this listener's own Skarbiec, and say how it ended: the
/// payload's exit status with its last line, which names what it recovered,
/// what it refused on, or why Skarbiec stayed unready.
async fn crypto_repair_outcome() -> String {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-s")
        .env(
            "SKARBIEC_READY_URL",
            format!(
                "{}/readyz",
                crate::config::skarbiec_url().trim_end_matches('/')
            ),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return format!("the repair payload could not be started: {error}"),
    };
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        if let Err(error) = stdin
            .write_all(crate::cli::host::SKARBIEC_CRYPTO_REPAIR_PAYLOAD.as_bytes())
            .await
        {
            return format!("the repair payload could not be handed to the shell: {error}");
        }
    }
    let output = crate::wait::child_output_async(
        child,
        "the declared skarbiec crypto repair (skarbiec recover-daemons) on this host",
    )
    .await;
    match output {
        Ok(output) => {
            let said = String::from_utf8_lossy(&output.stderr);
            let printed = String::from_utf8_lossy(&output.stdout);
            let last = said
                .lines()
                .chain(printed.lines())
                .rfind(|line| !line.trim().is_empty());
            match last {
                Some(last) => format!("exit {}: {last}", output.status),
                None => format!("exit {}, and the payload said nothing", output.status),
            }
        }
        Err(error) => format!("the repair payload did not run to its end: {error}"),
    }
}

//! Stage two: render what is about to be installed, before the machine is
//! touched. [`InstallPlan`] is the whole of it — the label, the argument
//! vector [`exec`] resolves for the kind, the environment [`env`] builds, and
//! the plist or systemd body [`render`] writes out of the two.

pub mod env;
pub mod exec;
pub mod render;

use std::path::{Path, PathBuf};

use crate::deploy::local_install::artifact::Bins;
use crate::deploy::local_install::{label, systemd_unit, LocalOs};
use crate::deploy::{py_dict_repr, DeployError};

use self::env::install_env;
use self::exec::exec_args_for;
use self::render::{daemon_plist_text, plist_text, systemd_user_unit};

/// Everything needed to install one local service: the pure plan Python
/// computes inside `install_local` before touching the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub name: String,
    pub kind: String,
    pub os: LocalOs,
    pub label: String,
    pub exec_args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The account a system LaunchDaemon runs as, for a host with no
    /// per-login launchd domain to load an agent into. `None` is the ordinary
    /// per-login agent, and it is the only value on Linux.
    ///
    /// An always-on mac is the whole reason. `launchctl bootstrap gui/<uid>`
    /// cannot work where nobody logs in graphically, and the ladder this
    /// module walks instead — `user/<uid>`, `asuser gui/<uid>`, then a crontab
    /// entry — ends in a process with no unit behind it. Four units on the
    /// always-on mini sat in `/Users/charles/Library/LaunchAgents` and never
    /// loaded once, the active coordinator among them, which is why nothing
    /// reaped an expired worker lease for two days.
    /// [`crate::deploy::service::requires_daemon_domain`] answers it from the
    /// registry declaration.
    pub daemon: Option<String>,
}

impl InstallPlan {
    /// Python `f"Wisent Compute {kind} ({entry.name})"` (Linux unit
    /// Description).
    pub fn description(&self) -> String {
        format!("Wisent Compute {} ({})", self.kind, self.name)
    }

    /// The plist (Darwin) or unit (Linux) destination path under `home`.
    ///
    /// A daemon is the machine's job, not the home's: it lives in
    /// `/Library/LaunchDaemons` and `home` names only the account it runs as.
    pub fn unit_path(&self, home: &Path) -> PathBuf {
        if self.daemon.is_some() {
            return PathBuf::from("/Library/LaunchDaemons").join(format!("{}.plist", self.label));
        }
        match self.os {
            LocalOs::Darwin => home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{}.plist", self.label)),
            LocalOs::Linux => home
                .join(".config")
                .join("systemd")
                .join("user")
                .join(systemd_unit(&self.label)),
        }
    }

    /// The plist (Darwin) or unit (Linux) content for an account home.
    pub fn content(&self, home: &Path) -> String {
        let log = home
            .join(".stado")
            .join("logs")
            .join(format!("{}.log", self.label));
        if let Some(account) = self.daemon.as_deref() {
            return daemon_plist_text(&self.label, &self.exec_args, &self.env, &log, account);
        }
        match self.os {
            LocalOs::Darwin => plist_text(&self.label, &self.exec_args, &self.env, &log),
            LocalOs::Linux => systemd_user_unit(&self.description(), &self.exec_args, &self.env),
        }
    }

    /// Python's dry-run branch of `install_local`.
    pub fn dry_run_lines(&self) -> Vec<String> {
        vec![
            format!(
                "[dry-run] {}={} on {}",
                self.kind,
                self.name,
                self.os.python_name()
            ),
            format!("  exec: {}", self.exec_args.join(" ")),
            format!("  env:  {}", py_dict_repr(&self.env)),
        ]
    }
}

/// Build the [`InstallPlan`] for one (name, kind) pair.
///
/// Eight arguments, one over the lint's threshold, because `daemon` is the
/// eighth and the seven before it are each a separate fact about the host this
/// unit is being rendered for. Collapsing them into a struct would move the
/// same list one indirection away without removing a single caller decision.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    name: &str,
    kind: &str,
    os: LocalOs,
    home: &Path,
    bins: &Bins,
    hf_token: &str,
    wc_python: &str,
    daemon: Option<String>,
) -> Result<InstallPlan, DeployError> {
    Ok(InstallPlan {
        name: name.to_string(),
        kind: kind.to_string(),
        os,
        label: label(kind, name),
        exec_args: exec_args_for(bins, kind, name)?,
        env: install_env(home, kind, hf_token, wc_python),
        daemon,
    })
}

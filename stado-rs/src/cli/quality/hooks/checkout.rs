//! Stado's pre-push hook in one checkout: the script, where Git runs it from,
//! and reading, writing and removing it there.
//!
//! The hook goes in the checkout's own hooks directory, `<git-dir>/hooks`,
//! and only when Git runs hooks from there: a `core.hooksPath` (the
//! checkout's own, or a global dispatcher's) sends Git to another directory,
//! whose `pre-push` decides what a push runs, and Stado writes nothing into
//! it. A `pre-push` the checkout already had that is not Stado's is kept
//! beside it as `pre-push.chained` and run after the gate passes, with the
//! same ref updates, so a Git LFS upload or a repository's own hook still
//! runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;

/// The hook Git runs before a push.
const HOOK: &str = "pre-push";
/// The hook a checkout had before Stado's, which Stado's runs after its gate.
pub(super) const CHAINED: &str = "pre-push.chained";
/// The first two lines of every hook Stado writes: how its own is told from
/// any other, whatever the rest of the script says.
const HEADER: &str = "#!/bin/sh\n# stado quality hooks: this checkout's pre-push gate.\n";

/// The hook script: Git's ref updates on stdin go to the installed Stado's
/// pre-push check, then to the chained hook when one is kept. The installed
/// Stado is named from `$HOME` at run time, as a fleet host's gate script
/// names it, so the hook neither carries this machine's home nor runs a
/// build that happened to write it.
pub(super) fn script() -> String {
    let stado = crate::deploy::host_exec::STADO_CLI
        .strip_prefix("~/")
        .unwrap_or(crate::deploy::host_exec::STADO_CLI);
    format!(
        "{HEADER}\
         # Written by `stado quality hooks install`; `stado quality hooks status` reads it\n\
         # and `stado quality hooks remove` takes it out, putting back the hook it chained.\n\
         # Git's ref updates go to `stado quality check --pre-push`, which checks each\n\
         # pushed commit's own tree and refuses the push when a declared gate refuses it;\n\
         # a push to main also runs the commit's lint gates on the fleet, unless the\n\
         # submit queue recorded that commit as its own landing. The pre-push hook this\n\
         # checkout had before, kept beside this one as {CHAINED}, then reads the\n\
         # same updates.\n\
         set -eu\n\
         updates=$(cat)\n\
         send() {{\n  if [ -n \"$updates\" ]; then printf '%s\\n' \"$updates\"; fi\n}}\n\
         send | \"$HOME\"/{} quality check --pre-push --if-declared\n\
         chained=\"${{0%/*}}/{CHAINED}\"\n\
         if [ -x \"$chained\" ]; then\n  send | \"$chained\" \"$@\"\nfi\n",
        crate::deploy::shlex_quote(stado)
    )
}

/// The installed Stado the hook runs, refused when this machine has none: a
/// hook naming a missing program refuses every push with "not found".
pub(super) fn installed_stado() -> Result<PathBuf, CmdError> {
    let home = crate::config_file::home().map_err(CmdError::declaration)?;
    let relative = crate::deploy::host_exec::STADO_CLI
        .strip_prefix("~/")
        .unwrap_or(crate::deploy::host_exec::STADO_CLI);
    let path = home.join(relative);
    if path.is_file() {
        return Ok(path);
    }
    Err(CmdError::missing(format!(
        "no Stado is installed at {}, the program the pre-push hook runs, so no hook was \
         written; install it with `stado product install --surface cli stado` and run this \
         again",
        path.display()
    )))
}

/// Where one checkout's hooks stand.
pub(super) struct Inspected {
    /// The checkout's own hooks directory, `<git-dir>/hooks`.
    pub(super) own: PathBuf,
    /// The directory Git runs hooks from for it; none when `core.hooksPath`
    /// is set to the empty string, which names no directory.
    pub(super) runs: Option<PathBuf>,
    /// The bytes of `pre-push` in the checkout's own hooks directory.
    pub(super) hook: Option<Vec<u8>>,
    /// Whether a chained hook is kept there.
    pub(super) chained: bool,
}

/// What a `pre-push` in the checkout's own directory is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// Git runs hooks from another directory (`core.hooksPath`).
    Governed,
    /// No `pre-push`.
    Absent,
    /// Stado's, exactly as this build writes it.
    Current,
    /// Stado's, as another build wrote it.
    Outdated,
    /// Not Stado's.
    Foreign,
}

impl Kind {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Governed => "governed",
            Self::Absent => "absent",
            Self::Current => "installed",
            Self::Outdated => "outdated",
            Self::Foreign => "foreign",
        }
    }
}

impl Inspected {
    pub(super) fn kind(&self) -> Kind {
        if self.runs.as_ref() != Some(&self.own) {
            return Kind::Governed;
        }
        match &self.hook {
            None => Kind::Absent,
            Some(bytes) if *bytes == script().into_bytes() => Kind::Current,
            Some(bytes) if bytes.starts_with(HEADER.as_bytes()) => Kind::Outdated,
            Some(_) => Kind::Foreign,
        }
    }

    /// Where `core.hooksPath` sends Git instead of the checkout's own hooks
    /// directory, in words.
    pub(super) fn elsewhere(&self) -> String {
        match &self.runs {
            Some(directory) => format!(
                "core.hooksPath sends Git to {}, whose {HOOK} decides what a push runs",
                directory.display()
            ),
            None => "core.hooksPath is set to the empty string, which names no directory, so Git \
                     runs no pre-push hook here"
                .to_string(),
        }
    }

    fn hook_path(&self) -> PathBuf {
        self.own.join(HOOK)
    }

    fn chained_path(&self) -> PathBuf {
        self.own.join(CHAINED)
    }
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::wait::output(
        Command::new("git")
            .args(args)
            .current_dir(checkout)
            .env("GIT_TERMINAL_PROMPT", "0"),
    )
    .map_err(|error| format!("git {} did not run: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The `core.hooksPath` Git reads for `checkout`, at whichever level it is
/// set; none when no level sets it. `git config --get` exits 1 for a key no
/// level sets, and any other failure is an error:
/// https://git-scm.com/docs/git-config#_description
fn configured_hooks_path(checkout: &Path) -> Result<Option<String>, String> {
    let shown = "git config --get core.hooksPath";
    let output = crate::wait::output(
        Command::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(checkout)
            .env("GIT_TERMINAL_PROMPT", "0"),
    )
    .map_err(|error| format!("{shown} did not run: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_owned(),
        )),
        Some(1) => Ok(None),
        _ => Err(format!(
            "{shown} in {} failed ({}): {}",
            checkout.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Read where `checkout`'s hooks stand.
pub(super) fn inspect(checkout: &Path) -> Result<Inspected, String> {
    let common = git(
        checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let own = PathBuf::from(common).join("hooks");
    let runs = match configured_hooks_path(checkout)?.as_deref() {
        None => Some(own.clone()),
        Some("") => None,
        Some(_) => Some(PathBuf::from(git(
            checkout,
            &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
        )?)),
    };
    let hook = match std::fs::read(own.join(HOOK)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", own.join(HOOK).display())),
    };
    let chained = own.join(CHAINED);
    let chained = chained
        .try_exists()
        .map_err(|error| format!("cannot read {}: {error}", chained.display()))?;
    Ok(Inspected {
        own,
        runs,
        hook,
        chained,
    })
}

fn renamed(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|error| {
        format!(
            "cannot move {} to {}: {error}",
            from.display(),
            to.display()
        )
    })
}

/// Write Stado's hook, through a file beside it renamed over it, so Git
/// never runs a half-written script.
fn write(inspected: &Inspected) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&inspected.own)
        .map_err(|error| format!("cannot create {}: {error}", inspected.own.display()))?;
    let staged = inspected
        .own
        .join(format!("{HOOK}.stado-{}", std::process::id()));
    std::fs::write(&staged, script())
        .map_err(|error| format!("cannot write {}: {error}", staged.display()))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("cannot make {} executable: {error}", staged.display()))?;
    renamed(&staged, &inspected.hook_path())
}

/// Put Stado's hook in place, answering what was done.
pub(super) fn install(inspected: &Inspected) -> Result<String, String> {
    match inspected.kind() {
        Kind::Governed => Ok(format!("left: {}", inspected.elsewhere())),
        Kind::Current => Ok("unchanged".to_string()),
        Kind::Absent => write(inspected).map(|()| "written".to_string()),
        Kind::Outdated => write(inspected).map(|()| "repaired".to_string()),
        Kind::Foreign if inspected.chained => Err(format!(
            "{} is not Stado's and {} is already kept beside it, so neither can take the other's \
             place; move one of them out of {} and install again",
            inspected.hook_path().display(),
            inspected.chained_path().display(),
            inspected.own.display()
        )),
        Kind::Foreign => {
            renamed(&inspected.hook_path(), &inspected.chained_path())?;
            write(inspected)
                .map(|()| format!("written; the hook it replaced runs after it as {CHAINED}"))
        }
    }
}

/// Take Stado's hook out and put back the hook it chained, answering what
/// was done.
pub(super) fn remove(inspected: &Inspected) -> Result<String, String> {
    match inspected.kind() {
        Kind::Governed => Ok(format!(
            "left: {}; Stado wrote no hook there",
            inspected.elsewhere()
        )),
        Kind::Foreign => Ok(format!(
            "left: {} is not Stado's",
            inspected.hook_path().display()
        )),
        Kind::Absent if inspected.chained => {
            renamed(&inspected.chained_path(), &inspected.hook_path())
                .map(|()| format!("restored the {CHAINED} hook as {HOOK}"))
        }
        Kind::Absent => Ok("nothing to remove".to_string()),
        Kind::Current | Kind::Outdated => {
            let hook = inspected.hook_path();
            std::fs::remove_file(&hook)
                .map_err(|error| format!("cannot remove {}: {error}", hook.display()))?;
            if inspected.chained {
                renamed(&inspected.chained_path(), &hook)?;
                return Ok(format!("removed; the {CHAINED} hook is {HOOK} again"));
            }
            Ok("removed".to_string())
        }
    }
}

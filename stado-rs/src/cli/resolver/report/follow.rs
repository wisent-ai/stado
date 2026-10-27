//! Bringing this host's resolver to a registry it does not hold yet: a
//! command that wrote the registry, or moved a route this host resolves
//! through, asks the resolver to read now (SIGUSR1, which its refresh loop
//! takes in place of the next tick) and waits until it holds the directory
//! generation or the exact store version written. Nothing is polled: the
//! kernel's change notification on the state file wakes the reader each time
//! the resolver publishes, and its next publication is its answer.

use super::published::{published_state, publisher_running, state_path, PublishedState};

/// What a follow waits for this host's resolver to hold: a directory
/// generation (or a later one), or the exact store version a write produced,
/// which also covers a write that changed no directory generation.
enum Mark {
    Generation(u64),
    Version(String),
}

impl Mark {
    fn held_by(&self, state: &PublishedState) -> bool {
        match self {
            Self::Generation(generation) => state.generation.is_some_and(|held| held >= *generation),
            Self::Version(version) => state.store_version.as_deref() == Some(version.as_str()),
        }
    }

    fn named(&self) -> String {
        match self {
            Self::Generation(generation) => format!("directory generation {generation}"),
            Self::Version(version) => format!("registry store version {version}"),
        }
    }

    /// What the resolver holds instead, in the same terms.
    fn instead(&self, state: &PublishedState) -> Option<String> {
        match self {
            Self::Generation(_) => state.generation.map(|held| format!("generation {held}")),
            Self::Version(_) => state.store_version.clone().map(|held| format!("store version {held}")),
        }
    }
}

/// Block until this host's resolver holds `generation` or a later one. A
/// command that just moved a route this host resolves through writes through
/// the new route only once its resolver follows; waiting a whole refresh
/// interval or racing it are both worse.
pub(crate) fn follow_generation(generation: u64) -> Result<(), String> {
    follow(Mark::Generation(generation))
}

/// [`follow_generation`] for the exact registry store version a write
/// produced.
pub(crate) fn follow_version(version: String) -> Result<(), String> {
    follow(Mark::Version(version))
}

/// Whether a resolver publishing `state` takes SIGUSR1: it does from the
/// moment it binds its listeners (the handler is installed just before), and
/// the signal would end one still starting.
fn takes_the_signal(state: &PublishedState) -> bool {
    state.listening
}

fn follow(mark: Mark) -> Result<(), String> {
    let path = state_path().ok_or_else(|| {
        "the resolver's state file has no location: HOME is unset and STADO_RESOLVER_STATE_FILE is empty"
            .to_string()
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?
        .to_path_buf();
    let state = published_state().ok_or_else(|| "no resolver has published its state on this host".to_string())?;
    if !publisher_running(&state) {
        return Err(format!("the resolver that published last (pid {}) is not running", state.pid));
    }
    if !takes_the_signal(&state) {
        return Err(format!(
            "the resolver (pid {}) is {} and binds no listener yet, so it cannot be asked to read now: {}",
            state.pid,
            state.state,
            match state.reason.as_deref() {
                Some(reason) => reason,
                None => "it published no reason",
            }
        ));
    }
    let pid = i32::try_from(state.pid).map_err(|error| format!("resolver pid {}: {error}", state.pid))?;
    // A publication written after the request is the resolver's answer to it,
    // unless that read moved the resolver to another source (a registry that
    // names another authority): then it is asked again, of the new source.
    let (mut asked_after, mut asked_from) = (state.updated_at.clone(), state.read_from.clone());
    let ask = || {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGUSR1)
            .map_err(|error| format!("the resolver (pid {pid}) could not be asked to read the registry now: {error}"))
    };
    ask()?;
    let named = mark.named();
    crate::wait::blocking(crate::wait::Kind::Process, format!("the resolver (pid {pid}) to hold {named}"), path.display(), || loop {
        let mut watch = crate::queue::local_file::watch_directories(std::slice::from_ref(&directory))
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if let Some(state) = published_state() {
            if mark.held_by(&state) && publisher_running(&state) {
                return Ok(());
            }
            // The publication after the request is the read it asked for:
            // without the mark, the registry that read reached does not
            // carry it yet, and waiting longer would only wait.
            if state.updated_at != asked_after && state.read_from != asked_from {
                (asked_after, asked_from) = (state.updated_at.clone(), state.read_from.clone());
                ask().map_err(std::io::Error::other)?;
            } else if state.updated_at != asked_after {
                let source = match state.read_from.as_deref() {
                    Some(authority) => format!("the authority {authority}"),
                    None => "this host's registry store".to_string(),
                };
                let answer = match (mark.instead(&state), state.reason.as_deref()) {
                    (_, Some(reason)) => reason.to_string(),
                    (Some(held), None) => format!("it holds {held}, so that registry does not carry {named} yet"),
                    (None, None) => "it published neither what it holds nor a reason".to_string(),
                };
                return Err(std::io::Error::other(format!(
                    "the resolver (pid {}) read the registry from {source} at the request and answered {} without \
                     holding {named}: {answer}",
                    state.pid, state.state
                )));
            }
        }
        watch.next().map_err(|error| std::io::Error::other(error.to_string()))?;
    })
    .map_err(|error| error.to_string())
}

/// After a registry write landed as store version `version`: this host's
/// resolver is brought to exactly that version, which covers a write that
/// advanced no directory generation (release control, a target's fields). On
/// the registry authority that resolver is what every other host reads
/// (`stado resolver snapshot` answers what it holds); on any host it is what
/// this host's next registry read takes first, so without this a second write
/// within one refresh interval reads the version before the first and is
/// refused as a conflict. A host with no running resolver, or one still
/// starting (its first read takes the registry as it stands), holds nothing
/// to bring forward. The write stands either way; a resolver that does not
/// follow is said on stderr with its own answer.
pub(crate) async fn follow_written(version: &str) {
    let Some(state) = published_state() else { return };
    if !publisher_running(&state) || !takes_the_signal(&state) {
        return;
    }
    let wanted = version.to_string();
    let followed = match tokio::task::spawn_blocking(move || follow_version(wanted)).await {
        Ok(followed) => followed,
        Err(error) => Err(format!("the wait did not end: {error}")),
    };
    if let Err(cause) = followed {
        eprintln!(
            "stado: the registry write landed as store version {version}, but this host's resolver did not follow it, \
             so readers asking it see the version before until it does: {cause}"
        );
    }
}

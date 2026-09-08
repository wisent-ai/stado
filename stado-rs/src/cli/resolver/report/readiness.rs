use serde_json::{json, Value};

use crate::service_resolution::{self, ResolverAdapter};
use crate::targets;

use crate::cli::{CmdError, CLICK_ERROR_CODE};

use crate::cli::resolver::directory::source::current_target;
use crate::cli::resolver::report::probe::{age_seconds, bind_listening, probe_authority};
use crate::cli::resolver::report::published::{
    published_state, state_path, RESOLVER_SERVING, RESOLVER_UNPUBLISHED, STATE_FILE,
};

/// `stado resolver status` — the four facts an operator needs about a local
/// resolver, and a non-zero exit when any of them is wrong.
///
/// The registry comes through [`targets::fetch_registry_or_last_good`]: a
/// command whose whole purpose is diagnosing a sick control plane must not die
/// with the authority it is diagnosing, and every host command did exactly
/// that on 2026-08-19. A cached answer is still an answer, and its age is a
/// blocker in the report rather than a footnote.
pub(crate) async fn status(target: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let (registry, notice) = targets::fetch_registry_or_last_good()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(notice) = notice.as_deref() {
        targets::report_registry_notice(notice);
    }
    let registry_staleness = registry.staleness_seconds;
    let document = registry.to_document();
    let published = published_state();
    let target = match target {
        Some(target) => target.to_string(),
        None => match published
            .as_ref()
            .map(|state| state.target.as_str())
            .filter(|target| !target.is_empty())
        {
            Some(target) => target.to_string(),
            None => current_target(&document).map_err(CmdError::click)?,
        },
    };
    let config =
        service_resolution::resolver_config(&document, &target).map_err(CmdError::click)?;
    let directory = service_resolution::directory(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::click("registry.service_directory is required"))?;

    // A declared bind is a loopback address *on the target*. Connecting to it
    // from here answers a different question: what this machine holds on that
    // number. The two answers were reported as one, and on a control-plane
    // host that runs its own resolver the numbers collide -- asking about
    // charless-mac-mini returned `listening: false` for three adapters that
    // one pid was holding there, and would have returned `listening: true`
    // for the four whose numbers this laptop happens to serve itself. So the
    // probe runs only where the binds live, and elsewhere reports that it did
    // not run rather than a measurement of the wrong socket.
    let local_target = current_target(&document).ok();
    let binds_are_local = local_target.as_deref() == Some(target.as_str());
    let api_listening = if binds_are_local {
        Some(bind_listening(&config.api_bind).await)
    } else {
        None
    };
    let mut probed: Vec<(&ResolverAdapter, Option<bool>)> =
        Vec::with_capacity(config.adapters.len());
    for adapter in &config.adapters {
        let listening = if binds_are_local {
            Some(bind_listening(&adapter.bind).await)
        } else {
            None
        };
        probed.push((adapter, listening));
    }
    let authority = probe_authority(&registry, &directory, &target, notice.as_deref()).await;

    let resolver_state = published
        .as_ref()
        .map(|state| state.state.as_str())
        .filter(|state| !state.is_empty())
        .unwrap_or(RESOLVER_UNPUBLISHED)
        .to_string();
    let held = published.as_ref().and_then(|state| state.generation);
    let held_age = published
        .as_ref()
        .and_then(|state| state.loaded_at.as_deref())
        .and_then(age_seconds);
    let behind = match (held, authority.generation) {
        (Some(held), Some(publishes)) if held < publishes => Some((held, publishes)),
        _ => None,
    };
    let past_window = held_age.is_some_and(|age| age > config.max_stale_seconds as i64);
    // Holding no generation at all counts: a resolver that has never loaded
    // the directory is not fresh, it is absent, and reporting `stale: false`
    // for it would vouch for a data plane that cannot resolve one name.
    let stale = held.is_none() || behind.is_some() || past_window;

    let mut blockers: Vec<String> = Vec::new();
    match &published {
        None => blockers.push(format!(
            "no resolver has published state at {}: nothing has served here since that file was \
             last removed",
            state_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| STATE_FILE.to_string())
        )),
        Some(state) if resolver_state != RESOLVER_SERVING => {
            let mut sentence = format!("the resolver reports state {resolver_state}");
            if let Some(reason) = state.reason.as_deref() {
                sentence.push_str(": ");
                sentence.push_str(reason);
            }
            if let Some(next) = state.next_attempt_at.as_deref() {
                sentence.push_str(&format!(
                    " (failed attempt {}, next read due {next})",
                    state.attempt
                ));
            }
            blockers.push(sentence);
        }
        Some(_) => {}
    }
    if api_listening == Some(false) {
        blockers.push(format!(
            "nothing is listening on the resolution API at {}",
            config.api_bind
        ));
    }
    for (adapter, listening) in &probed {
        if *listening == Some(false) {
            blockers.push(format!(
                "nothing is listening on the {} adapter for consumer {} at {}",
                adapter.service, adapter.consumer, adapter.bind
            ));
        }
    }
    if !authority.reachable {
        let mut sentence = format!(
            "the registry authority {} is unreachable",
            directory.authority.target
        );
        if let Some(detail) = authority.detail.as_deref() {
            sentence.push_str(": ");
            sentence.push_str(detail);
        }
        blockers.push(sentence);
    }
    if let Some((held, publishes)) = behind {
        blockers.push(format!(
            "the resolver holds service directory generation {held} and the authority publishes \
             {publishes}"
        ));
    }
    if past_window {
        blockers.push(format!(
            "the snapshot the resolver holds is {}s old, past the {}s max-stale window this \
             target declares",
            held_age.unwrap_or_default(),
            config.max_stale_seconds
        ));
    }
    if let Some(seconds) = registry_staleness {
        blockers.push(format!(
            "this answer read a registry copy {seconds}s old rather than the authority"
        ));
    }
    // The recovery copy is not advancing. Nothing is down yet, which is
    // exactly why it belongs here: the host looks healthy right up to the
    // outage the copy exists for, and then answers from whatever generation
    // it last accepted.
    if let Some(refusal) = published
        .as_ref()
        .and_then(|state| state.last_good_refusal.as_deref())
        .filter(|refusal| !refusal.is_empty())
    {
        blockers.push(format!(
            "the resolver is not refreshing this host's last-known-good registry copy \
             ({refusal}), so the fallback stays at the generation it last accepted"
        ));
    }

    // `down` is reserved for a resolver that is answering nothing at all.
    // Everything else that is wrong is `degraded`, because an adapter short of
    // its upstream still serves the services whose upstream is up.
    let verdict = if blockers.is_empty() {
        "ready"
    } else if api_listening == Some(false) && resolver_state != RESOLVER_SERVING {
        "down"
    } else {
        "degraded"
    };

    let report = json!({
        "target": target,
        "state": resolver_state,
        "pid": published.as_ref().map(|state| state.pid).filter(|pid| *pid != 0),
        "updated_at": published.as_ref().map(|state| state.updated_at.clone()),
        "api": {"bind": config.api_bind, "listening": api_listening},
        // A null `listening` is a measurement that did not happen, not a
        // socket that is down. Say which, so nobody reads the absence as
        // health or as an outage.
        "bind_probe": if binds_are_local {
            format!("probed: these binds are loopback addresses on {target}, which is this host")
        } else {
            format!(
                "not probed: these binds are loopback addresses on {target}, and this command \
                 ran on {}; ask that host with `stado host inventory {target}`",
                local_target.as_deref().unwrap_or("a host with no registry identity")
            )
        },
        "adapters": probed
            .iter()
            .map(|(adapter, listening)| json!({
                "service": adapter.service,
                "consumer": adapter.consumer,
                "bind": adapter.bind,
                "listening": listening,
            }))
            .collect::<Vec<Value>>(),
        "authority": {
            "target": directory.authority.target,
            "source": authority.source,
            "reachable": authority.reachable,
            "generation": authority.generation,
            "detail": authority.detail,
        },
        "generation": held,
        "generation_age_seconds": held_age,
        "max_stale_seconds": config.max_stale_seconds,
        "stale": stale,
        "registry_staleness_seconds": registry_staleness,
        "reason": published.as_ref().and_then(|state| state.reason.clone()),
        "attempt": published.as_ref().map_or(0, |state| state.attempt),
        "next_attempt_at": published.as_ref().and_then(|state| state.next_attempt_at.clone()),
        "verdict": verdict,
        "blockers": blockers,
    });

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "resolver {target} state={resolver_state} verdict={verdict} generation={} stale={}",
            held.map_or_else(|| "-".to_string(), |generation| generation.to_string()),
            if stale { "yes" } else { "no" }
        );
        println!(
            "api {} {}",
            config.api_bind,
            match api_listening {
                Some(true) => "listening",
                Some(false) => "not-listening",
                None => "not-probed",
            }
        );
        for (adapter, listening) in &probed {
            println!(
                "adapter {} consumer={} {} {}",
                adapter.service,
                adapter.consumer,
                adapter.bind,
                match listening {
                    Some(true) => "listening",
                    Some(false) => "not-listening",
                    None => "not-probed",
                }
            );
        }
        println!(
            "authority {} source={} {} generation={}",
            directory.authority.target,
            authority.source,
            if authority.reachable {
                "reachable"
            } else {
                "unreachable"
            },
            authority
                .generation
                .map_or_else(|| "-".to_string(), |generation| generation.to_string())
        );
        for blocker in &blockers {
            println!("blocker {blocker}");
        }
    }

    if verdict == "ready" {
        return Ok(());
    }
    // The report is the answer; a second `Error:` line restating it would be
    // the third copy of one fact on one screen.
    Err(CmdError::silent(CLICK_ERROR_CODE))
}

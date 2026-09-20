use super::*;

mod autologin;
mod install;
mod runtime;

pub(in crate::deploy::host_gui_automation) use autologin::reconcile_autologin;
pub(in crate::deploy::host_gui_automation) use install::reconcile_app;
pub(in crate::deploy::host_gui_automation) use runtime::reconcile_runtime;

/// Whether one user's session can be driven, and when it cannot, the item
/// that said so. A probe that only answered yes or no left the Developer ID
/// relay saying "not drivable" about a session that, asked directly, was;
/// the item is what makes the two answers comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReadiness {
    pub ready: bool,
    pub reason: Option<String>,
}

async fn session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    readiness_key: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    let report = status(target, password, runner).await;
    if let Some(error) = report.error {
        return Err(DeployError(error));
    }
    Ok(readiness_from_items(
        &report.items,
        expected_user,
        readiness_key,
    ))
}

/// The companion item that says WHY a readiness key is `no`. The status pass
/// records it beside the verdict and the verdict used to drop it: the
/// Developer ID relay refused for a day with `apple-challenge-ready is no`
/// while the probe's own sentence, one item away, named what stopped it.
fn readiness_error_key(readiness_key: &str) -> &'static str {
    match readiness_key {
        "apple-challenge-ready" => "apple-challenge-preflight-error",
        _ => "accessibility-error",
    }
}

/// One session's verdict, read from a status report's items.
fn readiness_from_items(
    items: &[(String, String)],
    expected_user: &str,
    readiness_key: &str,
) -> SessionReadiness {
    let value = |key: &str| {
        items
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    };
    let mut mismatches: Vec<String> = [
        ("console", expected_user),
        ("accessibility-user", expected_user),
        (readiness_key, "yes"),
    ]
    .into_iter()
    .filter(|(key, expected)| value(key) != Some(expected))
    .map(|(key, expected)| {
        format!(
            "{key} is {}, expected {expected}",
            value(key).unwrap_or("unreported")
        )
    })
    .collect();
    if !mismatches.is_empty() {
        if let Some(said) = value(readiness_error_key(readiness_key)) {
            mismatches.push(format!("{}: {said}", readiness_error_key(readiness_key)));
        }
    }
    SessionReadiness {
        ready: mismatches.is_empty(),
        reason: (!mismatches.is_empty()).then(|| mismatches.join("; ")),
    }
}

/// Whether CuaDriver can drive this exact user's current GUI session.
pub async fn automated_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    automated_session_readiness_for(target, expected_user, password, runner)
        .await
        .map(|verdict| verdict.ready)
}

/// [`automated_session_ready_for`], with the item that disagreed.
pub async fn automated_session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    session_readiness_for(target, expected_user, "gui-ready", password, runner).await
}

/// Whether the signed helper can read this exact user's Apple challenge.
pub async fn apple_challenge_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    apple_challenge_session_readiness_for(target, expected_user, password, runner)
        .await
        .map(|verdict| verdict.ready)
}

/// [`apple_challenge_session_ready_for`], with the item that disagreed.
pub async fn apple_challenge_session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    session_readiness_for(
        target,
        expected_user,
        "apple-challenge-ready",
        password,
        runner,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::readiness_from_items;

    fn items(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_matching_session_is_ready_and_says_nothing() {
        let verdict = readiness_from_items(
            &items(&[
                ("console", "lukaszbartoszcze"),
                ("accessibility-user", "lukaszbartoszcze"),
                ("apple-challenge-ready", "yes"),
            ]),
            "lukaszbartoszcze",
            "apple-challenge-ready",
        );
        assert!(verdict.ready);
        assert_eq!(verdict.reason, None);
    }

    /// The Developer ID relay refused all of 2026-09-19 and 2026-09-20 with
    /// `apple-challenge-ready is no`, while the probe's own sentence about
    /// what stopped it sat one item away in the same report.
    #[test]
    fn a_refused_session_carries_the_probes_own_error() {
        let verdict = readiness_from_items(
            &items(&[
                ("console", "lukaszbartoszcze"),
                ("accessibility-user", "lukaszbartoszcze"),
                ("apple-challenge-ready", "no"),
                (
                    "apple-challenge-preflight-error",
                    "helper exited 1: no Apple challenge window in this session",
                ),
            ]),
            "lukaszbartoszcze",
            "apple-challenge-ready",
        );
        assert!(!verdict.ready);
        let reason = verdict.reason.expect("a refusal carries a reason");
        assert!(reason.contains("apple-challenge-ready is no"), "{reason}");
        assert!(reason.contains("no Apple challenge window"), "{reason}");
    }

    /// A session belonging to somebody else names the user, and the GUI
    /// verdict takes its companion error from the accessibility probe.
    #[test]
    fn a_session_owned_by_another_user_names_it() {
        let verdict = readiness_from_items(
            &items(&[
                ("console", "charles"),
                ("accessibility-user", "charles"),
                ("gui-ready", "no"),
                ("accessibility-error", "CuaDriver daemon is not running"),
            ]),
            "controlyourai-relay",
            "gui-ready",
        );
        assert!(!verdict.ready);
        let reason = verdict.reason.expect("a refusal carries a reason");
        assert!(
            reason.contains("console is charles, expected controlyourai-relay"),
            "{reason}"
        );
        assert!(
            reason.contains("CuaDriver daemon is not running"),
            "{reason}"
        );
    }
}

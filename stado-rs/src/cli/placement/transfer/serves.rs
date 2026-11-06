//! Whether a source that answers on no SSH path still serves. SSH being
//! closed says nothing about the profile's units: a host whose sshd stopped,
//! or whose network refuses port 22 alone, can keep answering its clients,
//! and a take would then start a second copy here from older state, the two
//! writing diverging state. So before anything starts, each probe URL the
//! profile declares for the source is asked at every network name the
//! source is known by: the host of each SSH path and each declared host
//! name, with the URL's own port and path (a loopback probe URL names the
//! source's own interface, which is what those names reach from here).

use std::time::Duration;

use crate::cli::placement::candidates::profile_host;
use crate::cli::CmdError;
use crate::placement::PlacementProfile;
use crate::targets::ComputeTarget;

/// Every network name `source` is known by, each once.
fn names(source: &ComputeTarget) -> Vec<String> {
    let mut names: Vec<String> = source
        .ssh_connections()
        .map(|(_, destination)| crate::targets::ssh_hostname(destination))
        .chain(
            source
                .hostnames
                .iter()
                .map(|name| crate::targets::normalize_hostname(name)),
        )
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// `url` with its host replaced by `name`; `None` for a URL that does not
/// parse or whose host cannot be replaced.
fn at(url: &str, name: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    parsed.set_host(Some(name)).ok()?;
    Some(parsed.to_string())
}

/// The first `<service> at <url>` of `profile`'s probes that answers with
/// any HTTP status at `source`'s network names, each asked once within
/// `bound`; `None` when none answers. An HTTP answer of any status is a
/// unit serving: only a connection that fails or does not answer in time
/// counts as not serving.
pub(in crate::cli::placement::transfer) async fn still_serving(
    profile: &PlacementProfile,
    source: &ComputeTarget,
    bound: Duration,
) -> Result<Option<String>, CmdError> {
    let probes = &profile_host(profile, &source.name)?.probes;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(bound)
        .timeout(bound)
        .build()
        .map_err(CmdError::from)?;
    for name in names(source) {
        for probe in probes {
            let Some(url) = at(&probe.url, &name) else {
                return Err(CmdError::declaration(format!(
                    "probe URL {} of {} cannot be asked at {name}",
                    probe.url, probe.service
                )));
            };
            if client.get(&url).send().await.is_ok() {
                return Ok(Some(format!("{} at {url}", probe.service)));
            }
        }
    }
    Ok(None)
}

//! The proof: `/join.sh` fetched through the public address, and the byte count
//! that says it was this listener that answered.

use crate::cli::fleet::ingress::runtime::process::with_causes;
use crate::cli::fleet::ingress::{FETCH_TIMEOUT, POLL, PUBLIC_DEADLINE};

/// Fetch `/join.sh` through the public address and prove it is this listener's.
///
/// Two things are checked and both matter. A `200` says something answered the
/// route; the byte count says it answered with *the script this binary would
/// have served*, not with a captive portal, an error page or some other
/// deployment that happens to know the path. Retried until the edge has
/// propagated the new hostname, because a fresh quick tunnel is legitimately
/// unreachable for the first few seconds.
pub async fn verify_public(base: &str) -> Result<(usize, usize), String> {
    let expected = crate::dashboard::join_script_source().len();
    if expected == 0 {
        return Err(
            "this build embeds no deploy/join.sh, so there is nothing to verify the tunnel \
             against and the published address could not serve an invite anyway"
                .to_string(),
        );
    }
    let endpoint = format!("{base}/join.sh");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|exc| format!("could not build an HTTP client: {exc}"))?;
    let deadline = tokio::time::Instant::now() + PUBLIC_DEADLINE;
    loop {
        // Bound per attempt, so the deadline error carries the reason the LAST
        // fetch failed rather than the first.
        let last = match client.get(&endpoint).send().await {
            Ok(response) if response.status().as_u16() == 200 => {
                let served = response.bytes().await.map_err(|exc| {
                    format!("{endpoint} answered 200 but the body could not be read: {exc}")
                })?;
                if served.len() == expected {
                    return Ok((served.len(), expected));
                }
                return Err(format!(
                    "{endpoint} answered 200 with {} bytes, not the {expected} bytes this build \
                     serves at /join.sh: whatever is behind that address is not the enrollment \
                     listener this command started",
                    served.len()
                ));
            }
            Ok(response) => format!("HTTP {}", response.status().as_u16()),
            Err(exc) => with_causes(&exc.without_url()),
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "{endpoint} did not answer 200 from the internet within {}s (last: {last})",
                PUBLIC_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

//! The proof: `/join.sh` fetched through the public address, and the byte count
//! that says it was this listener that answered.

use crate::cli::fleet::ingress::runtime::process::with_causes;

/// Fetch `/join.sh` through the public address and prove it is this listener's.
///
/// Two things are checked and both matter. A `200` says something answered the
/// route; the byte count says it answered with *the script this binary would
/// have served*, not with a captive portal, an error page or some other
/// deployment that happens to know the path. The fetch is made once, and its
/// failure is reported with the transport's own causes or the status served.
pub async fn verify_public(base: &str) -> Result<(usize, usize), String> {
    let expected = crate::dashboard::join_script_source().len();
    if expected == 0 {
        return Err(
            "this build embeds no deploy/join fragments, so there is nothing to verify the tunnel \
             against and the published address could not serve an invite anyway"
                .to_string(),
        );
    }
    let endpoint = format!("{base}/join.sh");
    let response = reqwest::get(&endpoint).await.map_err(|exc| {
        format!(
            "{endpoint} could not be fetched from the internet: {}",
            with_causes(&exc.without_url())
        )
    })?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "{endpoint} answered HTTP {} from the internet, not 200",
            response.status()
        ));
    }
    let served = response
        .bytes()
        .await
        .map_err(|exc| format!("{endpoint} answered 200 but the body could not be read: {exc}"))?;
    if served.len() == expected {
        return Ok((served.len(), expected));
    }
    Err(format!(
        "{endpoint} answered 200 with {} bytes, not the {expected} bytes this build serves at \
         /join.sh: whatever is behind that address is not the enrollment listener this command \
         started",
        served.len()
    ))
}
